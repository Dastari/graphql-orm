//! Resolver catalogue: relation and entity resolvers of one semantic catalogue.
//!
//! [`crate::GraphqlOperationCatalog`] and the operations of a
//! [`GraphqlSemanticCatalog`] describe root fields only. A relation field
//! resolves below an object, and a Federation entity resolver is reached
//! through `_entities`, so neither has a root coordinate. A consumer that
//! binds policy to root fields therefore has no view of them.
//! [`GraphqlResolverCatalog`] lists both.
//!
//! It is a separate, versioned and fingerprinted document, bound to one
//! semantic catalogue by that catalogue's fingerprint. Composing it changes no
//! existing descriptor, wire format or fingerprint.
//!
//! The document is descriptive. It grants no resolver, field, row, database
//! or provider authority, and the server never reads it back. It names only
//! what the bound semantic catalogue and the schema's SDL already publish:
//! public type, field and argument names, relationship targets and
//! cardinalities, `@key` field sets, and the names of generated root fields.
//! It holds no scope name, policy text, table, column, relationship condition
//! or Rust path. An authorization requirement is given only as a reference to
//! a root field the semantic catalogue already lists.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    GraphqlEntitySemanticMetadata, GraphqlSemanticCatalog, GraphqlSemanticError,
    GraphqlSemanticOperationSource, GraphqlSemanticRelationshipCardinality, hex_sha256,
    valid_fingerprint, valid_graphql_name,
};
use crate::{GeneratedGraphqlOperationCategory, GraphqlOperationKind, rfc8785_canonical_json};

/// Current resolver-catalogue wire version.
pub const GRAPHQL_RESOLVER_CATALOG_VERSION: u16 = 1;

/// Router descriptor extension carrying a resolver catalogue.
pub const GRAPHQL_RESOLVER_CATALOG_EXTENSION_NAME: &str = "graphql-orm.resolver-catalog";

/// Stable resolver-catalogue fingerprint algorithm: SHA-256 over the RFC 8785
/// canonical JSON of the versioned document without its `fingerprint` member.
pub const GRAPHQL_RESOLVER_CATALOG_FINGERPRINT_ALGORITHM: &str =
    "graphql-orm-resolver-catalog-rfc8785-sha256-v1";

/// Name of the generated global identity field, which forms a Federation
/// `@key` of its own. It is the one key field a resolver catalogue may name
/// without the semantic catalogue listing it as an entity field.
pub const GRAPHQL_RESOLVER_NODE_ID_KEY_FIELD: &str = "NodeId";

const MAXIMUM_RELATIONS: usize = 65_536;
const MAXIMUM_ENTITY_KEYS: usize = 64;
const MAXIMUM_KEY_FIELDS: usize = 16;

/// Relation and entity resolvers of one semantic catalogue.
///
/// Compose it with [`Self::compose`]. A deserialized document is a trusted
/// host assertion like the catalogue itself; [`Self::validate_against`]
/// checks it against the catalogue it claims to describe, including that it
/// lists every relationship field of that catalogue exactly once.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlResolverCatalog {
    /// Wire version.
    pub version: u16,
    /// Fingerprint of the semantic catalogue this document describes.
    pub semantic_catalog_fingerprint: String,
    /// Every relationship field, ordered by owning type and field name.
    pub relations: Vec<GraphqlRelationResolverDescriptor>,
    /// Types with a resolvable Federation key, ordered by type name.
    pub entities: Vec<GraphqlEntityResolverDescriptor>,
    /// Stable fingerprint over the versioned document.
    pub fingerprint: String,
}

/// One relationship field resolver.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlRelationResolverDescriptor {
    /// Public GraphQL name of the type that owns the field.
    pub type_name: String,
    /// Public relationship field name.
    pub field_name: String,
    /// Whether the ORM generated the resolver or the host wrote it.
    pub source: GraphqlSemanticOperationSource,
    /// Public GraphQL name of the related type.
    pub target: String,
    /// Relationship cardinality.
    pub cardinality: GraphqlSemanticRelationshipCardinality,
    /// Public argument names, in declaration order. Their types are in the
    /// semantic catalogue.
    pub arguments: Vec<String>,
    /// Whether the field has an authoritative field policy.
    pub has_field_policy: bool,
    /// What a traversal is required to hold.
    pub authorization: GraphqlResolverAuthorization,
}

/// The Federation entity resolvers of one type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlEntityResolverDescriptor {
    /// Public GraphQL type name: the `__typename` of a representation.
    pub type_name: String,
    /// Resolvable keys in declaration order, one per `@key`.
    pub keys: Vec<GraphqlEntityResolverKey>,
    /// What an entity lookup is required to hold.
    pub authorization: GraphqlResolverAuthorization,
}

/// One resolvable Federation `@key`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlEntityResolverKey {
    /// Public field names of the key, in `@key(fields: ...)` order.
    pub fields: Vec<String>,
}

impl GraphqlEntityResolverKey {
    /// Creates one key from its field names.
    pub fn new<I, S>(fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            fields: fields.into_iter().map(Into::into).collect(),
        }
    }
}

/// The static authorization requirement of a relation or entity resolver.
///
/// The requirement is never spelled out here. It is either the requirement of
/// a root field the semantic catalogue lists, or it is not published.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum GraphqlResolverAuthorization {
    /// The resolver enforces the static requirement declared for this exposed
    /// generated root field of the related (or resolved) entity: the same
    /// authentication mode and the same fixed scopes. A consumer that knows
    /// that root's requirement, for example from
    /// [`crate::GraphqlOperationCatalog`], knows this one. The subgraph
    /// applies more on top: the authentication mode of the type that owns a
    /// relationship field, and field, row and database policy.
    SameAsRoot {
        /// Root operation kind; always `query`.
        kind: GraphqlOperationKind,
        /// Exact public root field name.
        field_name: String,
    },
    /// The requirement is enforced by the subgraph and is not published: the
    /// resolver is handwritten, the matching root is not exposed, the
    /// requirement binds root arguments this resolver does not have, or it
    /// differs from the root's.
    SubgraphOnly,
}

/// Generated resolver facts for one entity, as input to
/// [`GraphqlResolverCatalog::compose`].
///
/// Every root named here is checked against the semantic catalogue. A root
/// that is not an exposed generated root of the right category for the entity
/// is ignored, and the requirement is reported as
/// [`GraphqlResolverAuthorization::SubgraphOnly`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct GraphqlGeneratedResolverSource {
    /// Public GraphQL entity name, as in the semantic catalogue.
    pub entity_name: String,
    /// Relationship fields of this entity whose resolver the ORM generated.
    pub relation_fields: Vec<String>,
    /// Resolvable Federation keys in declaration order.
    pub federation_keys: Vec<Vec<String>>,
    /// The list root whose requirement a to-many traversal into this entity
    /// enforces, when the two are the same.
    pub to_many_root: Option<String>,
    /// The single-read root whose requirement a to-one traversal into this
    /// entity enforces, when the two are the same.
    pub to_one_root: Option<String>,
    /// The single-read root whose requirement this entity's Federation entity
    /// resolvers enforce, when the two are the same.
    pub entity_root: Option<String>,
}

impl GraphqlGeneratedResolverSource {
    /// Creates facts for one entity with no relation, key or root.
    pub fn new(entity_name: impl Into<String>) -> Self {
        Self {
            entity_name: entity_name.into(),
            ..Self::default()
        }
    }

    /// Sets the generated relationship fields.
    #[must_use]
    pub fn with_relation_fields(mut self, relation_fields: Vec<String>) -> Self {
        self.relation_fields = relation_fields;
        self
    }

    /// Sets the resolvable Federation keys.
    #[must_use]
    pub fn with_federation_keys(mut self, federation_keys: Vec<Vec<String>>) -> Self {
        self.federation_keys = federation_keys;
        self
    }

    /// Sets the roots whose requirements traversals into this entity enforce.
    #[must_use]
    pub fn with_traversal_roots(
        mut self,
        to_many_root: Option<String>,
        to_one_root: Option<String>,
    ) -> Self {
        self.to_many_root = to_many_root;
        self.to_one_root = to_one_root;
        self
    }

    /// Sets the root whose requirement the entity resolvers enforce.
    #[must_use]
    pub fn with_entity_root(mut self, entity_root: Option<String>) -> Self {
        self.entity_root = entity_root;
        self
    }
}

impl GraphqlResolverCatalog {
    /// Composes the resolver catalogue of one semantic catalogue.
    ///
    /// Every relationship field of the catalogue becomes one relation
    /// descriptor. A field is `generated` when `sources` lists it for its
    /// entity, and `custom` otherwise. Sources naming an entity the catalogue
    /// does not publish are ignored.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalogue is invalid, an entity has two
    /// sources, or a key does not validate against the catalogue.
    pub fn compose(
        catalog: &GraphqlSemanticCatalog,
        sources: impl IntoIterator<Item = GraphqlGeneratedResolverSource>,
    ) -> Result<Self, GraphqlSemanticError> {
        catalog.validate()?;
        let sources = sources.into_iter().collect::<Vec<_>>();
        let mut names = BTreeSet::new();
        if !sources
            .iter()
            .all(|source| names.insert(source.entity_name.as_str()))
        {
            return Err(GraphqlSemanticError::new(
                "resolver catalogue entity source is duplicated",
            ));
        }
        let source = |entity_name: &str| {
            sources
                .iter()
                .find(|source| source.entity_name == entity_name)
        };
        let published = |root: &Option<String>, entity_name: &str, category| {
            root.as_ref()
                .filter(|root| generated_query_root(catalog, root, entity_name, category))
                .map_or(GraphqlResolverAuthorization::SubgraphOnly, |root| {
                    GraphqlResolverAuthorization::SameAsRoot {
                        kind: GraphqlOperationKind::Query,
                        field_name: root.clone(),
                    }
                })
        };

        let mut relations = Vec::new();
        let mut entities = Vec::new();
        for entity in &catalog.entities {
            let owner = source(&entity.entity_name);
            for field in &entity.fields {
                let Some(relationship) = &field.relationship else {
                    continue;
                };
                let generated =
                    owner.is_some_and(|owner| owner.relation_fields.contains(&field.field_name));
                let authorization = match source(&relationship.target) {
                    Some(target) if generated => match relationship.cardinality {
                        GraphqlSemanticRelationshipCardinality::Many => published(
                            &target.to_many_root,
                            &relationship.target,
                            GeneratedGraphqlOperationCategory::List,
                        ),
                        GraphqlSemanticRelationshipCardinality::One => published(
                            &target.to_one_root,
                            &relationship.target,
                            GeneratedGraphqlOperationCategory::SingleRead,
                        ),
                    },
                    _ => GraphqlResolverAuthorization::SubgraphOnly,
                };
                relations.push(GraphqlRelationResolverDescriptor {
                    type_name: entity.entity_name.clone(),
                    field_name: field.field_name.clone(),
                    source: if generated {
                        GraphqlSemanticOperationSource::Generated
                    } else {
                        GraphqlSemanticOperationSource::Custom
                    },
                    target: relationship.target.clone(),
                    cardinality: relationship.cardinality,
                    arguments: relationship
                        .arguments
                        .iter()
                        .map(|argument| argument.graphql_name.clone())
                        .collect(),
                    has_field_policy: field.has_field_policy,
                    authorization,
                });
            }
            if let Some(owner) = owner.filter(|owner| !owner.federation_keys.is_empty()) {
                entities.push(GraphqlEntityResolverDescriptor {
                    type_name: entity.entity_name.clone(),
                    keys: owner
                        .federation_keys
                        .iter()
                        .map(|fields| GraphqlEntityResolverKey::new(fields.iter().cloned()))
                        .collect(),
                    authorization: published(
                        &owner.entity_root,
                        &entity.entity_name,
                        GeneratedGraphqlOperationCategory::SingleRead,
                    ),
                });
            }
        }
        relations.sort_by(|left, right| {
            (&left.type_name, &left.field_name).cmp(&(&right.type_name, &right.field_name))
        });
        entities.sort_by(|left, right| left.type_name.cmp(&right.type_name));
        let mut resolvers = Self {
            version: GRAPHQL_RESOLVER_CATALOG_VERSION,
            semantic_catalog_fingerprint: catalog.fingerprint.clone(),
            relations,
            entities,
            fingerprint: String::new(),
        };
        resolvers.fingerprint = resolvers.compute_fingerprint()?;
        resolvers.validate_against(catalog)?;
        Ok(resolvers)
    }

    /// Validates the document's shape, ordering and fingerprint.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported version, malformed names,
    /// non-canonical ordering, bounds violations or a stale fingerprint.
    pub fn validate(&self) -> Result<(), GraphqlSemanticError> {
        if self.version != GRAPHQL_RESOLVER_CATALOG_VERSION {
            return Err(GraphqlSemanticError::new(
                "resolver catalogue version is unsupported",
            ));
        }
        if !valid_fingerprint(&self.semantic_catalog_fingerprint) {
            return Err(GraphqlSemanticError::new(
                "resolver catalogue binding is invalid",
            ));
        }
        if self.relations.len() > MAXIMUM_RELATIONS
            || self.relations.windows(2).any(|pair| {
                (&pair[0].type_name, &pair[0].field_name)
                    >= (&pair[1].type_name, &pair[1].field_name)
            })
            || self
                .entities
                .windows(2)
                .any(|pair| pair[0].type_name >= pair[1].type_name)
        {
            return Err(GraphqlSemanticError::new(
                "resolver catalogue ordering is not canonical",
            ));
        }
        for relation in &self.relations {
            let unique = relation.arguments.iter().collect::<BTreeSet<_>>();
            if !valid_graphql_name(&relation.type_name)
                || !valid_graphql_name(&relation.field_name)
                || !valid_graphql_name(&relation.target)
                || !relation
                    .arguments
                    .iter()
                    .all(|name| valid_graphql_name(name))
                || unique.len() != relation.arguments.len()
                || !authorization_shape_valid(&relation.authorization)
                || (relation.source == GraphqlSemanticOperationSource::Custom
                    && relation.authorization != GraphqlResolverAuthorization::SubgraphOnly)
            {
                return Err(GraphqlSemanticError::new(
                    "resolver catalogue relation is invalid",
                ));
            }
        }
        for entity in &self.entities {
            let mut key_sets = BTreeSet::new();
            let keys_valid = entity.keys.iter().all(|key| {
                let unique = key.fields.iter().collect::<BTreeSet<_>>();
                !key.fields.is_empty()
                    && key.fields.len() <= MAXIMUM_KEY_FIELDS
                    && key.fields.iter().all(|name| valid_graphql_name(name))
                    && unique.len() == key.fields.len()
                    && key_sets.insert(unique)
            });
            if !valid_graphql_name(&entity.type_name)
                || entity.keys.is_empty()
                || entity.keys.len() > MAXIMUM_ENTITY_KEYS
                || !keys_valid
                || !authorization_shape_valid(&entity.authorization)
            {
                return Err(GraphqlSemanticError::new(
                    "resolver catalogue entity resolver is invalid",
                ));
            }
        }
        if !valid_fingerprint(&self.fingerprint) || self.compute_fingerprint()? != self.fingerprint
        {
            return Err(GraphqlSemanticError::new(
                "resolver catalogue fingerprint is stale",
            ));
        }
        Ok(())
    }

    /// Validates the document and checks it against the catalogue it is bound
    /// to.
    ///
    /// # Errors
    ///
    /// Returns an error when [`Self::validate`] fails, the catalogue
    /// fingerprint differs, the relations are not exactly the catalogue's
    /// relationship fields with the same target, cardinality, arguments and
    /// field-policy flag, a key names a field the catalogue does not publish
    /// as a readable plain field, or an authorization reference does not name
    /// an exposed generated root of the right entity and category.
    pub fn validate_against(
        &self,
        catalog: &GraphqlSemanticCatalog,
    ) -> Result<(), GraphqlSemanticError> {
        self.validate()?;
        if self.semantic_catalog_fingerprint != catalog.fingerprint {
            return Err(GraphqlSemanticError::new(
                "resolver catalogue describes another semantic catalogue",
            ));
        }
        let mismatch = || {
            GraphqlSemanticError::new("resolver catalogue relation does not match the catalogue")
        };
        let mut expected = catalog
            .entities
            .iter()
            .flat_map(|entity| {
                entity
                    .fields
                    .iter()
                    .filter(|field| field.relationship.is_some())
                    .map(move |field| (entity.entity_name.as_str(), field))
            })
            .collect::<Vec<_>>();
        expected.sort_by(|left, right| {
            (left.0, left.1.field_name.as_str()).cmp(&(right.0, right.1.field_name.as_str()))
        });
        if expected.len() != self.relations.len() {
            return Err(mismatch());
        }
        for (relation, (entity_name, field)) in self.relations.iter().zip(expected) {
            let relationship = field.relationship.as_ref().ok_or_else(mismatch)?;
            let root_valid = match &relation.authorization {
                GraphqlResolverAuthorization::SubgraphOnly => true,
                GraphqlResolverAuthorization::SameAsRoot { field_name, .. } => {
                    generated_query_root(
                        catalog,
                        field_name,
                        &relationship.target,
                        match relationship.cardinality {
                            GraphqlSemanticRelationshipCardinality::Many => {
                                GeneratedGraphqlOperationCategory::List
                            }
                            GraphqlSemanticRelationshipCardinality::One => {
                                GeneratedGraphqlOperationCategory::SingleRead
                            }
                        },
                    )
                }
            };
            if relation.type_name != entity_name
                || relation.field_name != field.field_name
                || relation.target != relationship.target
                || relation.cardinality != relationship.cardinality
                || relation.has_field_policy != field.has_field_policy
                || !relation.arguments.iter().eq(relationship
                    .arguments
                    .iter()
                    .map(|argument| &argument.graphql_name))
                || !root_valid
            {
                return Err(mismatch());
            }
        }
        for entity in &self.entities {
            let mismatch = || {
                GraphqlSemanticError::new(
                    "resolver catalogue entity resolver does not match the catalogue",
                )
            };
            let semantic = catalog
                .entities
                .iter()
                .find(|semantic| semantic.entity_name == entity.type_name)
                .ok_or_else(mismatch)?;
            let keys_published = entity.keys.iter().all(|key| {
                key.fields == [GRAPHQL_RESOLVER_NODE_ID_KEY_FIELD]
                    || key
                        .fields
                        .iter()
                        .all(|name| key_field_published(semantic, name))
            });
            let root_valid = match &entity.authorization {
                GraphqlResolverAuthorization::SubgraphOnly => true,
                GraphqlResolverAuthorization::SameAsRoot { field_name, .. } => {
                    generated_query_root(
                        catalog,
                        field_name,
                        &entity.type_name,
                        GeneratedGraphqlOperationCategory::SingleRead,
                    )
                }
            };
            if !keys_published || !root_valid {
                return Err(mismatch());
            }
        }
        Ok(())
    }

    /// Returns the descriptor of one relationship field.
    pub fn relation(
        &self,
        type_name: &str,
        field_name: &str,
    ) -> Option<&GraphqlRelationResolverDescriptor> {
        self.relations
            .iter()
            .find(|relation| relation.type_name == type_name && relation.field_name == field_name)
    }

    /// Returns the entity resolvers of one type, when it has a resolvable key.
    pub fn entity(&self, type_name: &str) -> Option<&GraphqlEntityResolverDescriptor> {
        self.entities
            .iter()
            .find(|entity| entity.type_name == type_name)
    }

    /// Encodes the validated document as RFC 8785 canonical JSON, for a
    /// build-time export beside the semantic catalogue.
    ///
    /// # Errors
    ///
    /// Returns an error when validation fails.
    pub fn export_json(&self) -> Result<String, GraphqlSemanticError> {
        let failed = || GraphqlSemanticError::new("resolver catalogue encoding failed");
        let bytes = rfc8785_canonical_json(&self.extension_payload()?).map_err(|_| failed())?;
        String::from_utf8(bytes).map_err(|_| failed())
    }

    /// Decodes and validates an exported document.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, unsupported or stale payloads.
    pub fn from_export_json(value: &str) -> Result<Self, GraphqlSemanticError> {
        let resolvers: Self = serde_json::from_str(value)
            .map_err(|_| GraphqlSemanticError::new("resolver catalogue payload is invalid"))?;
        resolvers.validate()?;
        Ok(resolvers)
    }

    /// Encodes a strict extension payload.
    ///
    /// # Errors
    ///
    /// Returns an error when validation fails.
    pub fn extension_payload(&self) -> Result<serde_json::Value, GraphqlSemanticError> {
        self.validate()?;
        serde_json::to_value(self)
            .map_err(|_| GraphqlSemanticError::new("resolver catalogue encoding failed"))
    }

    /// Decodes and validates an extension payload.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, unsupported or stale payloads.
    pub fn from_extension_payload(
        payload: serde_json::Value,
    ) -> Result<Self, GraphqlSemanticError> {
        let resolvers: Self = serde_json::from_value(payload)
            .map_err(|_| GraphqlSemanticError::new("resolver catalogue payload is invalid"))?;
        resolvers.validate()?;
        Ok(resolvers)
    }

    /// Creates the generic router-protocol extension carrier.
    ///
    /// The extension is opt-in: a descriptor carries it only when the host
    /// adds it. Whoever receives that descriptor reads every name in it.
    ///
    /// # Errors
    ///
    /// Returns an error when validation or protocol validation fails.
    #[cfg(feature = "router-protocol")]
    pub fn router_protocol_extension(
        &self,
    ) -> Result<graphql_orm_router_protocol::DescriptorExtension, GraphqlSemanticError> {
        graphql_orm_router_protocol::DescriptorExtension::new(
            GRAPHQL_RESOLVER_CATALOG_EXTENSION_NAME,
            GRAPHQL_RESOLVER_CATALOG_VERSION,
            self.extension_payload()?,
        )
        .map_err(|_| GraphqlSemanticError::new("resolver catalogue extension is invalid"))
    }

    fn compute_fingerprint(&self) -> Result<String, GraphqlSemanticError> {
        #[derive(Serialize)]
        struct FingerprintInput<'a> {
            algorithm: &'static str,
            version: u16,
            semantic_catalog_fingerprint: &'a str,
            relations: &'a [GraphqlRelationResolverDescriptor],
            entities: &'a [GraphqlEntityResolverDescriptor],
        }
        let failed = || GraphqlSemanticError::new("resolver catalogue encoding failed");
        let value = serde_json::to_value(FingerprintInput {
            algorithm: GRAPHQL_RESOLVER_CATALOG_FINGERPRINT_ALGORITHM,
            version: self.version,
            semantic_catalog_fingerprint: &self.semantic_catalog_fingerprint,
            relations: &self.relations,
            entities: &self.entities,
        })
        .map_err(|_| failed())?;
        Ok(hex_sha256(
            &rfc8785_canonical_json(&value).map_err(|_| failed())?,
        ))
    }
}

fn authorization_shape_valid(authorization: &GraphqlResolverAuthorization) -> bool {
    match authorization {
        GraphqlResolverAuthorization::SubgraphOnly => true,
        GraphqlResolverAuthorization::SameAsRoot { kind, field_name } => {
            *kind == GraphqlOperationKind::Query && valid_graphql_name(field_name)
        }
    }
}

/// Whether `root` is an exposed generated query root of `entity_name` in
/// `category`.
fn generated_query_root(
    catalog: &GraphqlSemanticCatalog,
    root: &str,
    entity_name: &str,
    category: GeneratedGraphqlOperationCategory,
) -> bool {
    catalog.operations.iter().any(|operation| {
        operation.kind == GraphqlOperationKind::Query
            && operation.field_name == root
            && operation.is_exposed
            && operation.source == GraphqlSemanticOperationSource::Generated
            && operation.generated_category == Some(category)
            && operation.generated_entity_name.as_deref() == Some(entity_name)
    })
}

/// A key field the catalogue publishes: a selectable field of the entity that
/// is not a relationship and has no field policy.
fn key_field_published(entity: &GraphqlEntitySemanticMetadata, field_name: &str) -> bool {
    entity.fields.iter().any(|field| {
        field.field_name == field_name
            && field.selectable
            && field.relationship.is_none()
            && !field.has_field_policy
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{
        GraphqlSemanticArgumentDescriptor, GraphqlSemanticClassification,
        GraphqlSemanticCollectionBound, GraphqlSemanticExport, GraphqlSemanticFieldMetadata,
        GraphqlSemanticOperationDescriptor, GraphqlSemanticRelationshipDescriptor,
        GraphqlSemanticTypeKind, GraphqlSemanticTypeRef, canonical_json_bytes,
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

    fn scalar(name: &str) -> GraphqlSemanticTypeRef {
        GraphqlSemanticTypeRef::named(name, GraphqlSemanticTypeKind::Scalar, false)
    }

    fn object(name: &str, nullable: bool) -> GraphqlSemanticTypeRef {
        GraphqlSemanticTypeRef::named(name, GraphqlSemanticTypeKind::Object, nullable)
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

    fn to_one(name: &str, target: &str) -> GraphqlSemanticFieldMetadata {
        let mut relation = field(name, object(target, true));
        relation.relationship = Some(GraphqlSemanticRelationshipDescriptor {
            target: target.into(),
            cardinality: GraphqlSemanticRelationshipCardinality::One,
            arguments: Vec::new(),
            collection_bound: None,
        });
        relation
    }

    fn to_many(name: &str, target: &str) -> GraphqlSemanticFieldMetadata {
        let mut relation = field(
            name,
            GraphqlSemanticTypeRef::list(false, Some(100), object(target, false)),
        );
        relation.relationship = Some(GraphqlSemanticRelationshipDescriptor {
            target: target.into(),
            cardinality: GraphqlSemanticRelationshipCardinality::Many,
            arguments: vec![GraphqlSemanticArgumentDescriptor {
                graphql_name: "Page".into(),
                description: "Page size.".into(),
                type_ref: GraphqlSemanticTypeRef::named(
                    "Int",
                    GraphqlSemanticTypeKind::Scalar,
                    true,
                ),
            }],
            collection_bound: Some(GraphqlSemanticCollectionBound::pageable("Page")),
        });
        relation
    }

    fn root(
        category: GeneratedGraphqlOperationCategory,
        entity_name: &str,
        field_name: &str,
        result: GraphqlSemanticTypeRef,
    ) -> GraphqlSemanticOperationDescriptor {
        GraphqlSemanticOperationDescriptor::generated(
            GraphqlOperationKind::Query,
            category,
            entity_name,
            field_name,
            format!("Read {entity_name} records."),
            Vec::new(),
            result,
            true,
        )
        .expect("root descriptor")
    }

    /// `Parent` has many `Child`; `Child` has one `Parent` and one handwritten
    /// `Summary` relation. `Summary` is a handwritten type.
    fn catalog() -> GraphqlSemanticCatalog {
        let mut guarded = to_one("Guarded", "Parent");
        guarded.has_field_policy = true;
        let mut policy_key = field("PolicyKey", scalar("String"));
        policy_key.has_field_policy = true;
        let parent = entity(
            "Parent",
            vec![
                field("Id", scalar("String")),
                field("Code", scalar("String")),
                policy_key,
                to_many("Children", "Child"),
            ],
        );
        let child = entity(
            "Child",
            vec![
                field("Id", scalar("String")),
                to_one("Parent", "Parent"),
                guarded,
                to_one("Summary", "Summary"),
            ],
        );
        let summary = entity("Summary", vec![field("Total", scalar("Int"))]);
        GraphqlSemanticCatalog::compose_descriptors(
            [parent, child, summary],
            [
                root(
                    GeneratedGraphqlOperationCategory::SingleRead,
                    "Parent",
                    "Parent",
                    object("Parent", true),
                ),
                root(
                    GeneratedGraphqlOperationCategory::List,
                    "Parent",
                    "Parents",
                    object("ParentConnection", false),
                ),
                root(
                    GeneratedGraphqlOperationCategory::List,
                    "Child",
                    "Children",
                    object("ChildConnection", false),
                ),
            ],
        )
        .expect("catalogue")
    }

    fn sources() -> Vec<GraphqlGeneratedResolverSource> {
        vec![
            GraphqlGeneratedResolverSource::new("Parent")
                .with_relation_fields(vec!["Children".into()])
                .with_federation_keys(vec![
                    vec!["Id".into()],
                    vec!["Code".into()],
                    vec![GRAPHQL_RESOLVER_NODE_ID_KEY_FIELD.into()],
                ])
                .with_traversal_roots(Some("Parents".into()), Some("Parent".into()))
                .with_entity_root(Some("Parent".into())),
            GraphqlGeneratedResolverSource::new("Child")
                .with_relation_fields(vec!["Parent".into(), "Guarded".into()])
                .with_traversal_roots(Some("Children".into()), Some("Child".into())),
        ]
    }

    fn same_as(root: &str) -> GraphqlResolverAuthorization {
        GraphqlResolverAuthorization::SameAsRoot {
            kind: GraphqlOperationKind::Query,
            field_name: root.into(),
        }
    }

    fn composed() -> GraphqlResolverCatalog {
        GraphqlResolverCatalog::compose(&catalog(), sources()).expect("resolver catalogue")
    }

    /// Recomputes the fingerprint after a test edits the document.
    fn resealed(mut resolvers: GraphqlResolverCatalog) -> GraphqlResolverCatalog {
        resolvers.fingerprint = resolvers.compute_fingerprint().unwrap();
        resolvers
    }

    #[test]
    fn lists_every_relationship_field_and_each_resolvable_key() {
        let resolvers = composed();
        assert_eq!(resolvers.version, GRAPHQL_RESOLVER_CATALOG_VERSION);
        assert_eq!(
            resolvers
                .relations
                .iter()
                .map(|relation| (relation.type_name.as_str(), relation.field_name.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Child", "Guarded"),
                ("Child", "Parent"),
                ("Child", "Summary"),
                ("Parent", "Children"),
            ]
        );
        let children = resolvers.relation("Parent", "Children").unwrap();
        assert_eq!(children.target, "Child");
        assert_eq!(
            children.cardinality,
            GraphqlSemanticRelationshipCardinality::Many
        );
        assert_eq!(children.arguments, ["Page"]);
        assert_eq!(children.source, GraphqlSemanticOperationSource::Generated);
        assert_eq!(children.authorization, same_as("Children"));

        let parent = resolvers.relation("Child", "Parent").unwrap();
        assert_eq!(parent.authorization, same_as("Parent"));
        assert!(!parent.has_field_policy);
        assert!(
            resolvers
                .relation("Child", "Guarded")
                .unwrap()
                .has_field_policy
        );

        let entity = resolvers.entity("Parent").unwrap();
        assert_eq!(
            entity.keys,
            [
                GraphqlEntityResolverKey::new(["Id"]),
                GraphqlEntityResolverKey::new(["Code"]),
                GraphqlEntityResolverKey::new(["NodeId"]),
            ]
        );
        assert_eq!(entity.authorization, same_as("Parent"));
        assert!(resolvers.entity("Child").is_none());
        assert!(resolvers.relation("Parent", "Id").is_none());
    }

    #[test]
    fn handwritten_and_unmatched_resolvers_publish_no_requirement() {
        let resolvers = composed();
        // The host wrote `Child.Summary`; its target has no generated source.
        let summary = resolvers.relation("Child", "Summary").unwrap();
        assert_eq!(summary.source, GraphqlSemanticOperationSource::Custom);
        assert_eq!(
            summary.authorization,
            GraphqlResolverAuthorization::SubgraphOnly
        );

        // `Child` exposes no single-read root, and a named root of the wrong
        // category or entity is not a reference.
        let mut sources = sources();
        sources[0].to_one_root = Some("Parents".into());
        sources[0].to_many_root = None;
        sources[0].entity_root = Some("Children".into());
        sources[1].to_many_root = Some("Missing".into());
        let resolvers = GraphqlResolverCatalog::compose(&catalog(), sources).unwrap();
        for (type_name, field_name) in [("Child", "Parent"), ("Parent", "Children")] {
            assert_eq!(
                resolvers
                    .relation(type_name, field_name)
                    .unwrap()
                    .authorization,
                GraphqlResolverAuthorization::SubgraphOnly
            );
        }
        assert_eq!(
            resolvers.entity("Parent").unwrap().authorization,
            GraphqlResolverAuthorization::SubgraphOnly
        );

        // With no source at all, every relation is handwritten.
        let bare = GraphqlResolverCatalog::compose(&catalog(), []).unwrap();
        assert_eq!(bare.relations.len(), 4);
        assert!(bare.entities.is_empty());
        assert!(bare.relations.iter().all(|relation| {
            relation.source == GraphqlSemanticOperationSource::Custom
                && relation.authorization == GraphqlResolverAuthorization::SubgraphOnly
        }));
    }

    #[test]
    fn composition_is_deterministic_and_round_trips() {
        let resolvers = composed();
        let mut reversed = sources();
        reversed.reverse();
        assert_eq!(
            resolvers,
            GraphqlResolverCatalog::compose(&catalog(), reversed).unwrap()
        );
        assert_eq!(resolvers.fingerprint.len(), 64);

        let exported = resolvers.export_json().unwrap();
        assert_eq!(exported, resolvers.export_json().unwrap());
        let decoded = GraphqlResolverCatalog::from_export_json(&exported).unwrap();
        assert_eq!(decoded, resolvers);
        decoded.validate_against(&catalog()).unwrap();

        let payload = resolvers.extension_payload().unwrap();
        assert_eq!(
            GraphqlResolverCatalog::from_extension_payload(payload).unwrap(),
            resolvers
        );
        // The document holds no fraction, large integer or non-ASCII key, so
        // its RFC 8785 bytes equal the earlier sorted encoding.
        assert_eq!(
            exported.as_bytes(),
            canonical_json_bytes(&serde_json::to_value(&resolvers).unwrap())
        );
    }

    #[test]
    fn golden_export_is_stable() {
        let catalog = GraphqlSemanticCatalog::compose_descriptors(
            [
                entity("Parent", vec![field("Id", scalar("String"))]),
                entity(
                    "Child",
                    vec![field("Id", scalar("String")), to_one("Parent", "Parent")],
                ),
            ],
            [root(
                GeneratedGraphqlOperationCategory::SingleRead,
                "Parent",
                "Parent",
                object("Parent", true),
            )],
        )
        .unwrap();
        let resolvers = GraphqlResolverCatalog::compose(
            &catalog,
            [
                GraphqlGeneratedResolverSource::new("Parent")
                    .with_federation_keys(vec![vec!["Id".into()]])
                    .with_traversal_roots(None, Some("Parent".into()))
                    .with_entity_root(Some("Parent".into())),
                GraphqlGeneratedResolverSource::new("Child")
                    .with_relation_fields(vec!["Parent".into()]),
            ],
        )
        .unwrap();
        let expected = format!(
            concat!(
                r#"{{"entities":[{{"authorization":{{"field_name":"Parent","kind":"query","mode":"same_as_root"}},"#,
                r#""keys":[{{"fields":["Id"]}}],"type_name":"Parent"}}],"fingerprint":"{}","#,
                r#""relations":[{{"arguments":[],"authorization":{{"field_name":"Parent","kind":"query","mode":"same_as_root"}},"#,
                r#""cardinality":"one","field_name":"Parent","has_field_policy":false,"source":"generated","#,
                r#""target":"Parent","type_name":"Child"}}],"semantic_catalog_fingerprint":"{}","version":1}}"#,
            ),
            resolvers.fingerprint, catalog.fingerprint,
        );
        assert_eq!(resolvers.export_json().unwrap(), expected);
        assert_eq!(
            GRAPHQL_RESOLVER_CATALOG_FINGERPRINT_ALGORITHM,
            "graphql-orm-resolver-catalog-rfc8785-sha256-v1"
        );
    }

    #[test]
    fn composing_resolvers_changes_no_semantic_catalogue_byte() {
        let before = catalog();
        let bytes = serde_json::to_vec(&before).unwrap();
        let fingerprint = before.fingerprint.clone();
        let _ = composed();
        let after = catalog();
        assert_eq!(serde_json::to_vec(&after).unwrap(), bytes);
        assert_eq!(after.fingerprint, fingerprint);
        before.validate().unwrap();
    }

    #[test]
    fn refuses_stale_rebound_and_unsupported_documents() {
        let catalog = catalog();
        let resolvers = composed();

        let mut stale = resolvers.clone();
        stale.relations[0].has_field_policy = false;
        assert!(stale.validate().is_err());

        let mut unsupported = resolvers.clone();
        unsupported.version = 2;
        assert!(resealed(unsupported).validate().is_err());

        let other = GraphqlSemanticCatalog::compose_descriptors(
            [entity("Other", vec![field("Id", scalar("String"))])],
            [],
        )
        .unwrap();
        assert!(resolvers.validate_against(&other).is_err());
        resolvers.validate_against(&catalog).unwrap();

        let exported = resolvers.export_json().unwrap();
        assert!(
            GraphqlResolverCatalog::from_export_json(
                &exported.replace("\"version\":1", "\"version\":1,\"scopes\":[]")
            )
            .is_err()
        );
        assert!(GraphqlResolverCatalog::from_export_json("{}").is_err());
    }

    #[test]
    fn refuses_a_document_that_disagrees_with_the_catalogue() {
        let catalog = catalog();
        let edits: [fn(&mut GraphqlResolverCatalog); 9] = [
            // A relationship field is left out.
            |resolvers| {
                resolvers.relations.pop();
            },
            |resolvers| resolvers.relations[1].target = "Child".into(),
            |resolvers| {
                resolvers.relations[1].cardinality = GraphqlSemanticRelationshipCardinality::Many;
            },
            |resolvers| resolvers.relations[3].arguments.clear(),
            // A field policy is hidden.
            |resolvers| resolvers.relations[0].has_field_policy = false,
            // The reference names a root of another entity.
            |resolvers| resolvers.relations[1].authorization = same_as("Children"),
            |resolvers| resolvers.entities[0].authorization = same_as("Parents"),
            // A key names a field with a field policy, then an unknown field.
            |resolvers| resolvers.entities[0].keys[1].fields = vec!["PolicyKey".into()],
            |resolvers| resolvers.entities[0].type_name = "Summary".into(),
        ];
        for (index, edit) in edits.into_iter().enumerate() {
            let mut edited = composed();
            edit(&mut edited);
            let edited = resealed(edited);
            assert!(
                edited.validate_against(&catalog).is_err(),
                "edit {index} was accepted"
            );
        }
    }

    #[test]
    fn refuses_malformed_shapes() {
        let edits: [fn(&mut GraphqlResolverCatalog); 8] = [
            |resolvers| resolvers.relations.swap(0, 1),
            |resolvers| resolvers.relations[0].field_name = "not a name".into(),
            // A handwritten resolver cannot claim a root's requirement.
            |resolvers| resolvers.relations[2].authorization = same_as("Parent"),
            |resolvers| {
                resolvers.relations[1].authorization = GraphqlResolverAuthorization::SameAsRoot {
                    kind: GraphqlOperationKind::Mutation,
                    field_name: "Parent".into(),
                };
            },
            |resolvers| resolvers.entities[0].keys.clear(),
            |resolvers| resolvers.entities[0].keys[0].fields.clear(),
            // Two keys over one field set.
            |resolvers| resolvers.entities[0].keys[1].fields = vec!["Id".into()],
            |resolvers| resolvers.semantic_catalog_fingerprint = "abc".into(),
        ];
        for (index, edit) in edits.into_iter().enumerate() {
            let mut edited = composed();
            edit(&mut edited);
            assert!(
                resealed(edited).validate().is_err(),
                "edit {index} was accepted"
            );
        }
    }

    #[test]
    fn refuses_duplicate_sources_and_unpublished_keys() {
        let catalog = catalog();
        let mut duplicated = sources();
        duplicated.push(GraphqlGeneratedResolverSource::new("Parent"));
        assert!(GraphqlResolverCatalog::compose(&catalog, duplicated).is_err());

        for key in ["PolicyKey", "Children", "Unknown"] {
            let source = GraphqlGeneratedResolverSource::new("Parent")
                .with_federation_keys(vec![vec![key.into()]]);
            assert!(
                GraphqlResolverCatalog::compose(&catalog, [source]).is_err(),
                "{key} was accepted as a key"
            );
        }
        // A source for an entity the catalogue does not publish is ignored.
        let unknown = GraphqlGeneratedResolverSource::new("Unlisted")
            .with_federation_keys(vec![vec!["Id".into()]]);
        let resolvers = GraphqlResolverCatalog::compose(&catalog, [unknown]).unwrap();
        assert!(resolvers.entities.is_empty());
    }

    /// The document names public GraphQL identities and nothing else: no
    /// scope, policy text, description, table, column or Rust path.
    #[test]
    fn export_holds_only_names_the_semantic_catalogue_and_sdl_publish() {
        fn strings(value: &serde_json::Value, output: &mut Vec<String>) {
            match value {
                serde_json::Value::String(string) => output.push(string.clone()),
                serde_json::Value::Array(values) => {
                    values.iter().for_each(|value| strings(value, output));
                }
                serde_json::Value::Object(members) => {
                    members.values().for_each(|value| strings(value, output));
                }
                _ => {}
            }
        }
        let catalog = catalog();
        let resolvers = composed();
        let mut allowed = BTreeSet::from([
            "generated".to_owned(),
            "custom".to_owned(),
            "one".to_owned(),
            "many".to_owned(),
            "query".to_owned(),
            "same_as_root".to_owned(),
            "subgraph_only".to_owned(),
            GRAPHQL_RESOLVER_NODE_ID_KEY_FIELD.to_owned(),
            catalog.fingerprint.clone(),
            resolvers.fingerprint.clone(),
        ]);
        for entity in &catalog.entities {
            allowed.insert(entity.entity_name.clone());
            for field in &entity.fields {
                allowed.insert(field.field_name.clone());
                for argument in field.relationship.iter().flat_map(|r| &r.arguments) {
                    allowed.insert(argument.graphql_name.clone());
                }
            }
        }
        for operation in &catalog.operations {
            allowed.insert(operation.field_name.clone());
        }
        let mut found = Vec::new();
        strings(&resolvers.extension_payload().unwrap(), &mut found);
        assert!(!found.is_empty());
        for string in found {
            assert!(allowed.contains(&string), "unexpected string {string:?}");
        }
    }

    #[cfg(feature = "router-protocol")]
    #[test]
    fn router_extension_carries_the_versioned_document() {
        let resolvers = composed();
        let extension = resolvers.router_protocol_extension().unwrap();
        let encoded = serde_json::to_value(&extension).unwrap();
        let text = encoded.to_string();
        assert!(text.contains(GRAPHQL_RESOLVER_CATALOG_EXTENSION_NAME));
        assert!(text.contains(&resolvers.fingerprint));
    }
}
