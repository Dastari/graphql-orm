//! Resolver catalogue: relation and entity resolvers of one semantic catalogue.
//!
//! [`crate::GraphqlOperationCatalog`] and the operations of a
//! [`GraphqlSemanticCatalog`] describe root fields only. A relation field
//! resolves below an object, and a Federation entity resolver is reached
//! through `_entities`, so neither has a root coordinate. A consumer that
//! binds policy to root fields therefore has no view of them.
//! [`GraphqlResolverCatalog`] lists both, for the resolvers the ORM
//! generates. Handwritten relationship fields and handwritten entity
//! resolvers are out of scope for version 1: nothing proves what they resolve
//! or enforce, so a served field is listed only when its generated resolver
//! is proven mounted.
//!
//! It is a separate, versioned and fingerprinted document, bound to one
//! semantic catalogue by that catalogue's fingerprint. Composing it changes no
//! existing descriptor, wire format or fingerprint.
//!
//! Coordinates are those of the served schema. A relationship the semantic
//! catalogue describes is listed only when the served schema has that field,
//! under the type name the schema registers. Where that name differs from the
//! semantic catalogue's entity name, the descriptor carries both, so it still
//! binds to the catalogue.
//!
//! The document is descriptive. It grants no resolver, field, row, database
//! or provider authority, and the server never reads it back. It names only
//! what the bound semantic catalogue and the schema's SDL already publish:
//! public type, field and argument names, relationship targets and
//! cardinalities, `@key` field sets, and the names of generated root fields.
//! It holds no scope name, policy text, table, column, relationship condition
//! or Rust path. An authorization requirement is given only as a reference to
//! a root field the semantic catalogue already lists.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    GraphqlEntitySemanticMetadata, GraphqlSemanticCatalog, GraphqlSemanticError,
    GraphqlSemanticOperationSource, GraphqlSemanticRelationshipCardinality,
    GraphqlSemanticRelationshipDescriptor, GraphqlSemanticTypeRef, MAXIMUM_ENTITIES,
    MAXIMUM_FIELDS_PER_ENTITY, hex_sha256, valid_fingerprint, valid_graphql_name,
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

/// Most fields one resolvable key may name, and most keys one type may
/// carry: the semantic catalogue's own bound on the fields of an entity.
/// Entity derives accept any key the entity's fields can form, so no key the
/// semantic catalogue can describe is refused here.
pub const GRAPHQL_RESOLVER_MAXIMUM_KEY_FIELDS: usize = MAXIMUM_FIELDS_PER_ENTITY;

const MAXIMUM_RELATIONS: usize = MAXIMUM_ENTITIES * MAXIMUM_FIELDS_PER_ENTITY;
const MAXIMUM_ENTITY_KEYS: usize = MAXIMUM_FIELDS_PER_ENTITY;

/// Relation and entity resolvers of one semantic catalogue.
///
/// Compose it with [`Self::compose`]. A deserialized document is a trusted
/// host assertion like the catalogue itself; [`Self::validate_against`]
/// checks every entry against the catalogue it claims to describe. It cannot
/// check that no served relationship is missing: the semantic catalogue does
/// not record which of its fields a schema serves.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlResolverCatalog {
    /// Wire version.
    pub version: u16,
    /// Fingerprint of the semantic catalogue this document describes.
    pub semantic_catalog_fingerprint: String,
    /// Every served relationship field with a generated resolver, ordered
    /// by owning type and field name.
    pub relations: Vec<GraphqlRelationResolverDescriptor>,
    /// Types with a resolvable Federation key, ordered by type name.
    pub entities: Vec<GraphqlEntityResolverDescriptor>,
    /// Stable fingerprint over the versioned document.
    pub fingerprint: String,
}

/// One generated relationship field resolver.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlRelationResolverDescriptor {
    /// Name the served schema registers for the type that owns the field.
    pub type_name: String,
    /// The owning entity's name in the semantic catalogue, when it differs
    /// from [`Self::type_name`]. Omitted from the wire when they are equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_entity_name: Option<String>,
    /// Relationship field name, as served and as in the semantic catalogue.
    pub field_name: String,
    /// Name the served schema registers for the related type.
    pub target: String,
    /// The related entity's name in the semantic catalogue, when it differs
    /// from [`Self::target`]. Omitted from the wire when they are equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_target: Option<String>,
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
    /// Name the served schema registers for the type: the `__typename` of a
    /// representation.
    pub type_name: String,
    /// The entity's name in the semantic catalogue, when it differs from
    /// [`Self::type_name`]. Omitted from the wire when they are equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_entity_name: Option<String>,
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
    /// matching root is not exposed, the related type is not a generated
    /// entity of this schema, the requirement binds root arguments this
    /// resolver does not have, or it differs from the root's.
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
    /// Entity name, as in the semantic catalogue.
    pub entity_name: String,
    /// Name the served schema registers for the entity's type, when it
    /// differs from [`Self::entity_name`].
    pub type_name: Option<String>,
    /// Whether the served schema registers no type for this entity. Such an
    /// entity has no coordinate: nothing of it is listed, even when the
    /// schema serves an unrelated type with the entity's name.
    pub unregistered: bool,
    /// Relationship fields of this entity whose generated resolver the
    /// schema mounts.
    pub relation_fields: Vec<String>,
    /// Federation keys the served schema resolves, in declaration order.
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

    /// Sets the name the served schema registers for the entity's type.
    #[must_use]
    pub fn with_type_name(mut self, type_name: impl Into<String>) -> Self {
        let type_name = type_name.into();
        self.type_name = (type_name != self.entity_name).then_some(type_name);
        self
    }

    /// Marks the entity as having no type in the served schema.
    #[must_use]
    pub fn unregistered(mut self) -> Self {
        self.unregistered = true;
        self
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

/// One field a schema serves, as input to
/// [`GraphqlResolverCatalog::compose`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphqlServedField {
    name: String,
    result_type: String,
    arguments: Vec<(String, String)>,
}

impl GraphqlServedField {
    /// Creates a field from its name and its GraphQL type as the schema
    /// spells it, for example `[Child!]!`. Only the named type is kept.
    pub fn new(name: impl Into<String>, graphql_type: &str) -> Self {
        Self {
            name: name.into(),
            result_type: graphql_type
                .chars()
                .filter(|character| !matches!(character, '[' | ']' | '!'))
                .collect(),
            arguments: Vec::new(),
        }
    }

    /// Adds one argument, with its GraphQL type as the schema spells it.
    #[must_use]
    pub fn with_argument(
        mut self,
        name: impl Into<String>,
        graphql_type: impl Into<String>,
    ) -> Self {
        self.arguments.push((name.into(), graphql_type.into()));
        self
    }
}

/// The object types a schema serves and the fields of each, as input to
/// [`GraphqlResolverCatalog::compose`].
///
/// Composition lists a coordinate only when it is present here, so an empty
/// value yields an empty resolver catalogue.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GraphqlServedObjects {
    objects: BTreeMap<String, BTreeMap<String, GraphqlServedField>>,
}

impl GraphqlServedObjects {
    /// Creates an empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one served object type and its fields.
    #[must_use]
    pub fn with_object(
        mut self,
        type_name: impl Into<String>,
        fields: impl IntoIterator<Item = GraphqlServedField>,
    ) -> Self {
        self.objects
            .entry(type_name.into())
            .or_default()
            .extend(fields.into_iter().map(|field| (field.name.clone(), field)));
        self
    }

    /// Whether the schema serves this object type.
    pub fn has_object(&self, type_name: &str) -> bool {
        self.objects.contains_key(type_name)
    }

    /// Whether the schema serves this field of this object type.
    pub fn has_field(&self, type_name: &str, field_name: &str) -> bool {
        self.field(type_name, field_name).is_some()
    }

    fn field(&self, type_name: &str, field_name: &str) -> Option<&GraphqlServedField> {
        self.objects.get(type_name)?.get(field_name)
    }

    /// Whether the served field is the relationship the catalogue describes:
    /// it returns `target`, directly or as the nodes of a connection, and
    /// takes exactly the catalogue's arguments with the same types.
    fn serves_relationship(
        &self,
        type_name: &str,
        field_name: &str,
        target: &str,
        relationship: &GraphqlSemanticRelationshipDescriptor,
    ) -> bool {
        let Some(field) = self.field(type_name, field_name) else {
            return false;
        };
        // A member of `object` named `name` under any naming rule.
        let member = |object: &str, name: &str| {
            self.objects.get(object).and_then(|fields| {
                fields
                    .values()
                    .find(|field| field.name.eq_ignore_ascii_case(name))
            })
        };
        let returns_target = field.result_type == target
            || (relationship.cardinality == GraphqlSemanticRelationshipCardinality::Many
                && member(&field.result_type, "edges")
                    .and_then(|edges| member(&edges.result_type, "node"))
                    .is_some_and(|node| node.result_type == target));
        returns_target
            && field.arguments.len() == relationship.arguments.len()
            && relationship.arguments.iter().all(|argument| {
                field.arguments.iter().any(|(name, graphql_type)| {
                    *name == argument.graphql_name
                        && *graphql_type == graphql_type_text(&argument.type_ref)
                })
            })
    }
}

/// A semantic type as GraphQL spells it, for example `[Child!]!`.
fn graphql_type_text(type_ref: &GraphqlSemanticTypeRef) -> String {
    match type_ref {
        GraphqlSemanticTypeRef::Named { name, nullable, .. } => {
            format!("{name}{}", if *nullable { "" } else { "!" })
        }
        GraphqlSemanticTypeRef::List { item, nullable, .. } => {
            format!(
                "[{}]{}",
                graphql_type_text(item),
                if *nullable { "" } else { "!" }
            )
        }
    }
}

impl GraphqlResolverCatalog {
    /// Composes the resolver catalogue of one semantic catalogue.
    ///
    /// A relationship field of the catalogue becomes a relation descriptor
    /// when `sources` lists it for its entity as a mounted generated
    /// resolver and `served` confirms it: the owning type has that field,
    /// the field returns the related type (directly, or as the nodes of a
    /// connection for a to-many relationship) and it takes exactly the
    /// catalogue's arguments with the same types. Every other relationship
    /// is left out, including any a handwritten resolver serves: the shape
    /// of a served field never makes it a relationship by itself. An entity
    /// is listed with its keys when `served` has its type. An entity whose
    /// source is
    /// [`unregistered`](GraphqlGeneratedResolverSource::unregistered) is
    /// never listed, as an owner, a target or a keyed type; an entity with
    /// no source is taken to be served under its catalogue name. Sources
    /// naming an entity the catalogue does not publish are ignored.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalogue is invalid, an entity has two
    /// sources, two entities are served under one type name, a key does not
    /// validate against the catalogue, or a bound is exceeded.
    pub fn compose(
        catalog: &GraphqlSemanticCatalog,
        sources: impl IntoIterator<Item = GraphqlGeneratedResolverSource>,
        served: &GraphqlServedObjects,
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

        // The served name of a semantic entity, and the semantic name to
        // carry beside it when the two differ.
        let served_name = |entity_name: &str| {
            let type_name = source(entity_name)
                .and_then(|source| source.type_name.clone())
                .unwrap_or_else(|| entity_name.to_owned());
            let semantic = (type_name != entity_name).then(|| entity_name.to_owned());
            (type_name, semantic)
        };

        let mut relations = Vec::new();
        let mut entities = Vec::new();
        for entity in &catalog.entities {
            let owner = source(&entity.entity_name);
            if owner.is_some_and(|owner| owner.unregistered) {
                continue;
            }
            let (type_name, semantic_entity_name) = served_name(&entity.entity_name);
            for field in &entity.fields {
                let Some(relationship) = &field.relationship else {
                    continue;
                };
                let (target, semantic_target) = served_name(&relationship.target);
                // Only a mounted generated resolver is listed. The checks
                // against the served schema confirm it; they never admit a
                // field on their own.
                if !owner.is_some_and(|owner| owner.relation_fields.contains(&field.field_name))
                    || source(&relationship.target).is_some_and(|target| target.unregistered)
                    || !served.has_object(&target)
                    || !served.serves_relationship(
                        &type_name,
                        &field.field_name,
                        &target,
                        relationship,
                    )
                {
                    continue;
                }
                let authorization = match source(&relationship.target) {
                    Some(target) => match relationship.cardinality {
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
                    None => GraphqlResolverAuthorization::SubgraphOnly,
                };
                relations.push(GraphqlRelationResolverDescriptor {
                    type_name: type_name.clone(),
                    semantic_entity_name: semantic_entity_name.clone(),
                    field_name: field.field_name.clone(),
                    target,
                    semantic_target,
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
            if let Some(owner) = owner
                .filter(|owner| !owner.federation_keys.is_empty() && served.has_object(&type_name))
            {
                entities.push(GraphqlEntityResolverDescriptor {
                    type_name,
                    semantic_entity_name,
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
                || !semantic_name_valid(&relation.semantic_entity_name, &relation.type_name)
                || !semantic_name_valid(&relation.semantic_target, &relation.target)
                || !relation
                    .arguments
                    .iter()
                    .all(|name| valid_graphql_name(name))
                || unique.len() != relation.arguments.len()
                || !authorization_shape_valid(&relation.authorization)
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
                    && key.fields.len() <= GRAPHQL_RESOLVER_MAXIMUM_KEY_FIELDS
                    && key.fields.iter().all(|name| valid_graphql_name(name))
                    && unique.len() == key.fields.len()
                    && key_sets.insert(unique)
            });
            if !valid_graphql_name(&entity.type_name)
                || !semantic_name_valid(&entity.semantic_entity_name, &entity.type_name)
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
    /// fingerprint differs, a relation is not a relationship field of the
    /// catalogue with the same target, cardinality, arguments and
    /// field-policy flag, a key names a field the catalogue does not publish
    /// as a readable plain field, an authorization reference does not name an
    /// exposed generated root of the right entity and category, or two
    /// entries disagree about which served type a catalogue entity is.
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
        let semantic_entity = |entity_name: &str| {
            catalog
                .entities
                .iter()
                .find(|entity| entity.entity_name == entity_name)
        };
        // One served type per catalogue entity, and one entity per type.
        let mut served_by_entity = BTreeMap::new();
        let mut entity_by_served = BTreeMap::new();
        let mut bind = |served: &'_ str, semantic: &'_ str| {
            *served_by_entity
                .entry(semantic.to_owned())
                .or_insert_with(|| served.to_owned())
                == served
                && *entity_by_served
                    .entry(served.to_owned())
                    .or_insert_with(|| semantic.to_owned())
                    == semantic
        };
        for relation in &self.relations {
            let entity_name = relation
                .semantic_entity_name
                .as_deref()
                .unwrap_or(&relation.type_name);
            let target_name = relation
                .semantic_target
                .as_deref()
                .unwrap_or(&relation.target);
            let field = semantic_entity(entity_name)
                .and_then(|entity| {
                    entity
                        .fields
                        .iter()
                        .find(|field| field.field_name == relation.field_name)
                })
                .ok_or_else(mismatch)?;
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
            if target_name != relationship.target
                || relation.cardinality != relationship.cardinality
                || relation.has_field_policy != field.has_field_policy
                || !relation.arguments.iter().eq(relationship
                    .arguments
                    .iter()
                    .map(|argument| &argument.graphql_name))
                || !root_valid
                || !bind(&relation.type_name, entity_name)
                || !bind(&relation.target, target_name)
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
            let entity_name = entity
                .semantic_entity_name
                .as_deref()
                .unwrap_or(&entity.type_name);
            let semantic = semantic_entity(entity_name).ok_or_else(mismatch)?;
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
                        entity_name,
                        GeneratedGraphqlOperationCategory::SingleRead,
                    )
                }
            };
            if !keys_published || !root_valid || !bind(&entity.type_name, entity_name) {
                return Err(mismatch());
            }
        }
        Ok(())
    }

    /// Returns the descriptor of one relationship field, by the names the
    /// served schema registers.
    pub fn relation(
        &self,
        type_name: &str,
        field_name: &str,
    ) -> Option<&GraphqlRelationResolverDescriptor> {
        self.relations
            .iter()
            .find(|relation| relation.type_name == type_name && relation.field_name == field_name)
    }

    /// Returns the entity resolvers of one served type, when it has a
    /// resolvable key.
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

/// A carried semantic name is a GraphQL name that differs from the served
/// one; an equal name is left out, so each document has one encoding.
fn semantic_name_valid(semantic: &Option<String>, served: &str) -> bool {
    semantic
        .as_deref()
        .is_none_or(|semantic| valid_graphql_name(semantic) && semantic != served)
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

    /// `Parent` has many `Child`; `Child` has one `Parent` and one `Summary`.
    /// `Summary` is a handwritten type with no generated source.
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
                .with_relation_fields(vec!["Parent".into(), "Guarded".into(), "Summary".into()])
                .with_traversal_roots(Some("Children".into()), Some("Child".into())),
        ]
    }

    fn same_as(root: &str) -> GraphqlResolverAuthorization {
        GraphqlResolverAuthorization::SameAsRoot {
            kind: GraphqlOperationKind::Query,
            field_name: root.into(),
        }
    }

    /// A schema that serves the catalogue's types and fields as declared.
    /// `type_name` gives each entity's served name, or `None` when the schema
    /// does not serve it; `keep` chooses the served fields.
    fn serving_with(
        catalog: &GraphqlSemanticCatalog,
        type_name: impl Fn(&str) -> Option<String>,
        keep: impl Fn(&str, &str) -> bool,
    ) -> GraphqlServedObjects {
        let served_field = |field: &GraphqlSemanticFieldMetadata| match &field.relationship {
            Some(relationship) => {
                let target =
                    type_name(&relationship.target).unwrap_or_else(|| relationship.target.clone());
                relationship.arguments.iter().fold(
                    GraphqlServedField::new(field.field_name.clone(), &target),
                    |served, argument| {
                        served.with_argument(
                            argument.graphql_name.clone(),
                            graphql_type_text(&argument.type_ref),
                        )
                    },
                )
            }
            None => GraphqlServedField::new(
                field.field_name.clone(),
                &graphql_type_text(&field.type_ref),
            ),
        };
        catalog
            .entities
            .iter()
            .fold(GraphqlServedObjects::new(), |served, entity| {
                let Some(name) = type_name(&entity.entity_name) else {
                    return served;
                };
                served.with_object(
                    name,
                    entity
                        .fields
                        .iter()
                        .filter(|field| keep(&entity.entity_name, &field.field_name))
                        .map(&served_field),
                )
            })
    }

    /// A schema that serves every type and field of the catalogue.
    fn serving(catalog: &GraphqlSemanticCatalog) -> GraphqlServedObjects {
        serving_with(catalog, |name| Some(name.to_owned()), |_, _| true)
    }

    fn compose(
        catalog: &GraphqlSemanticCatalog,
        sources: impl IntoIterator<Item = GraphqlGeneratedResolverSource>,
    ) -> Result<GraphqlResolverCatalog, GraphqlSemanticError> {
        GraphqlResolverCatalog::compose(catalog, sources, &serving(catalog))
    }

    fn composed() -> GraphqlResolverCatalog {
        compose(&catalog(), sources()).expect("resolver catalogue")
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
    fn unmatched_resolvers_publish_no_requirement_and_unproven_ones_are_omitted() {
        let resolvers = composed();
        // `Child.Summary` is generated, but its target has no generated
        // source, so there is no root to refer to.
        let summary = resolvers.relation("Child", "Summary").unwrap();
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
        let resolvers = compose(&catalog(), sources).unwrap();
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

        // A served relationship no source claims is not proven generated,
        // so it is omitted, whatever its shape.
        let mut partial = self::sources();
        partial[1].relation_fields = vec!["Parent".into()];
        let resolvers = compose(&catalog(), partial).unwrap();
        assert!(resolvers.relation("Child", "Parent").is_some());
        assert!(resolvers.relation("Child", "Guarded").is_none());
        assert!(resolvers.relation("Child", "Summary").is_none());
        assert!(serving(&catalog()).has_field("Child", "Summary"));

        // With no source at all, nothing is listed.
        let bare = compose(&catalog(), []).unwrap();
        assert!(bare.relations.is_empty());
        assert!(bare.entities.is_empty());
        bare.validate_against(&catalog()).unwrap();
    }

    #[test]
    fn composition_is_deterministic_and_round_trips() {
        let resolvers = composed();
        let mut reversed = sources();
        reversed.reverse();
        assert_eq!(resolvers, compose(&catalog(), reversed).unwrap());
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
        let resolvers = compose(
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
                r#""cardinality":"one","field_name":"Parent","has_field_policy":false,"#,
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
        let edits: [fn(&mut GraphqlResolverCatalog); 11] = [
            // A reference to a root of another entity than the target.
            |resolvers| resolvers.relations[2].authorization = same_as("Parent"),
            // A carried semantic name the catalogue does not publish.
            |resolvers| resolvers.relations[1].semantic_entity_name = Some("Unlisted".into()),
            // One catalogue entity under two served names.
            |resolvers| {
                resolvers.relations[3].type_name = "PublicParent".into();
                resolvers.relations[3].semantic_entity_name = Some("Parent".into());
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
        let edits: [fn(&mut GraphqlResolverCatalog); 11] = [
            // A carried semantic name must differ from the served one.
            |resolvers| resolvers.relations[0].semantic_entity_name = Some("Child".into()),
            |resolvers| resolvers.relations[0].semantic_target = Some("not a name".into()),
            |resolvers| resolvers.entities[0].semantic_entity_name = Some("Parent".into()),
            |resolvers| resolvers.relations.swap(0, 1),
            |resolvers| resolvers.relations[0].field_name = "not a name".into(),
            |resolvers| resolvers.relations[2].target = "not a name".into(),
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
        assert!(compose(&catalog, duplicated).is_err());

        for key in ["PolicyKey", "Children", "Unknown"] {
            let source = GraphqlGeneratedResolverSource::new("Parent")
                .with_federation_keys(vec![vec![key.into()]]);
            assert!(
                compose(&catalog, [source]).is_err(),
                "{key} was accepted as a key"
            );
        }
        // A source for an entity the catalogue does not publish is ignored.
        let unknown = GraphqlGeneratedResolverSource::new("Unlisted")
            .with_federation_keys(vec![vec!["Id".into()]]);
        let resolvers = compose(&catalog, [unknown]).unwrap();
        assert!(resolvers.entities.is_empty());
    }

    /// A relationship the catalogue describes but the schema does not serve
    /// has no coordinate, so it is not listed and no root is attached.
    #[test]
    fn lists_only_coordinates_the_schema_serves() {
        let catalog = catalog();
        let all = serving(&catalog);
        let without = |type_name: &str, dropped: Option<&str>| {
            serving_with(
                &catalog,
                |name| (dropped.is_some() || name != type_name).then(|| name.to_owned()),
                |entity, field| entity != type_name || Some(field) != dropped,
            )
        };
        assert!(all.has_field("Child", "Parent"));
        assert!(!all.has_field("Child", "Missing"));
        assert!(!all.has_object("Missing"));

        // The field is not served.
        let resolvers =
            GraphqlResolverCatalog::compose(&catalog, sources(), &without("Child", Some("Parent")))
                .unwrap();
        assert!(resolvers.relation("Child", "Parent").is_none());
        assert_eq!(resolvers.relations.len(), 3);
        resolvers.validate_against(&catalog).unwrap();
        assert!(
            !resolvers
                .export_json()
                .unwrap()
                .contains(r#""field_name":"Parent","has"#)
        );

        // The owning type is not served: none of its fields is.
        let resolvers =
            GraphqlResolverCatalog::compose(&catalog, sources(), &without("Child", None)).unwrap();
        assert!(
            resolvers
                .relations
                .iter()
                .all(|relation| relation.type_name != "Child" && relation.target != "Child")
        );
        // The entity's type is not served: its keys are not resolvable.
        let resolvers =
            GraphqlResolverCatalog::compose(&catalog, sources(), &without("Parent", None)).unwrap();
        assert!(resolvers.entities.is_empty());
        assert!(
            resolvers
                .relations
                .iter()
                .all(|relation| relation.target != "Parent")
        );

        // Nothing served: nothing listed.
        let empty =
            GraphqlResolverCatalog::compose(&catalog, sources(), &GraphqlServedObjects::new())
                .unwrap();
        assert!(empty.relations.is_empty() && empty.entities.is_empty());
        empty.validate_against(&catalog).unwrap();
    }

    /// A type the schema registers under another name is listed under that
    /// name, and carries its catalogue name for the binding.
    #[test]
    fn coordinates_use_the_registered_type_name() {
        let catalog = catalog();
        let mut sources = sources();
        sources[0] = sources[0].clone().with_type_name("PublicParent");
        // A name equal to the entity name is not a rename.
        sources[1] = sources[1].clone().with_type_name("Child");
        assert_eq!(sources[1].type_name, None);
        let served = serving_with(
            &catalog,
            |name| {
                Some(if name == "Parent" {
                    "PublicParent".to_owned()
                } else {
                    name.to_owned()
                })
            },
            |_, _| true,
        );
        let resolvers = GraphqlResolverCatalog::compose(&catalog, sources, &served).unwrap();
        resolvers.validate_against(&catalog).unwrap();

        let children = resolvers.relation("PublicParent", "Children").unwrap();
        assert_eq!(children.semantic_entity_name.as_deref(), Some("Parent"));
        assert_eq!(children.target, "Child");
        assert_eq!(children.semantic_target, None);
        assert_eq!(children.authorization, same_as("Children"));
        assert!(resolvers.relation("Parent", "Children").is_none());

        let parent = resolvers.relation("Child", "Parent").unwrap();
        assert_eq!(parent.target, "PublicParent");
        assert_eq!(parent.semantic_target.as_deref(), Some("Parent"));
        assert_eq!(parent.semantic_entity_name, None);
        assert_eq!(parent.authorization, same_as("Parent"));

        let entity = resolvers.entity("PublicParent").unwrap();
        assert_eq!(entity.semantic_entity_name.as_deref(), Some("Parent"));
        assert!(resolvers.entity("Parent").is_none());
        // Ordered by served name: `Child` relations, then `PublicParent`.
        assert_eq!(
            resolvers.relations.last().unwrap().type_name,
            "PublicParent"
        );

        let decoded =
            GraphqlResolverCatalog::from_export_json(&resolvers.export_json().unwrap()).unwrap();
        assert_eq!(decoded, resolvers);
        decoded.validate_against(&catalog).unwrap();

        // Without the served type, the renamed entity has no coordinate.
        let unserved = GraphqlResolverCatalog::compose(&catalog, self::sources(), &served).unwrap();
        assert!(unserved.entities.is_empty());
        assert!(unserved.relation("Parent", "Children").is_none());
    }

    /// A served field with the relationship's name is the relationship only
    /// when it returns the related type and takes the catalogue's arguments.
    #[test]
    fn a_same_named_field_of_another_shape_is_not_the_relationship() {
        let catalog = catalog();
        let listed = |served: &GraphqlServedObjects, type_name: &str, field: &str| {
            GraphqlResolverCatalog::compose(&catalog, sources(), served)
                .unwrap()
                .relation(type_name, field)
                .cloned()
        };
        let base = serving(&catalog);
        assert!(listed(&base, "Child", "Parent").is_some());
        assert!(listed(&base, "Parent", "Children").is_some());

        // `Child.Parent` is served, but as a string or as another object.
        for other in ["String", "Summary", "[Summary!]!"] {
            let served = base
                .clone()
                .with_object("Child", [GraphqlServedField::new("Parent", other)]);
            assert!(served.has_field("Child", "Parent"));
            assert!(listed(&served, "Child", "Parent").is_none(), "{other}");
            // Its other relationships are unaffected.
            assert!(listed(&served, "Child", "Guarded").is_some());
        }

        // `Parent.Children` returns the target but takes other arguments.
        let children = || GraphqlServedField::new("Children", "[Child!]!");
        for field in [
            children(),
            children().with_argument("Page", "String"),
            children().with_argument("Limit", "Int"),
            children()
                .with_argument("Page", "Int")
                .with_argument("Extra", "Int"),
        ] {
            let served = base.clone().with_object("Parent", [field]);
            assert!(listed(&served, "Parent", "Children").is_none());
        }

        // A to-many relationship may return a connection over the target,
        // under any naming rule, but not a connection over another type.
        let connection = |edges: &str, node: &str, node_type: &str| {
            base.clone()
                .with_object(
                    "Parent",
                    [GraphqlServedField::new("Children", "ChildConnection!")
                        .with_argument("Page", "Int")],
                )
                .with_object(
                    "ChildConnection",
                    [GraphqlServedField::new(edges, "[ChildEdge!]!")],
                )
                .with_object("ChildEdge", [GraphqlServedField::new(node, node_type)])
        };
        for (edges, node) in [("edges", "node"), ("Edges", "Node"), ("EDGES", "NODE")] {
            let relation = listed(&connection(edges, node, "Child!"), "Parent", "Children")
                .expect("a connection over the target");
            assert_eq!(relation.authorization, same_as("Children"));
        }
        assert!(
            listed(
                &connection("edges", "node", "Summary!"),
                "Parent",
                "Children"
            )
            .is_none()
        );
        assert!(listed(&connection("items", "node", "Child!"), "Parent", "Children").is_none());
    }

    /// An entity with a generated source that the schema does not register
    /// has no coordinate, even when the schema serves a type with its name.
    /// A catalogue type with no generated source is different: it is taken
    /// to be served under its catalogue name.
    #[test]
    fn an_unregistered_entity_is_not_matched_to_a_same_named_type() {
        let catalog = catalog();
        let served = serving(&catalog);
        assert!(served.has_object("Parent") && served.has_field("Parent", "Children"));
        let mut sources = sources();
        sources[0] = sources[0].clone().unregistered();
        let resolvers = GraphqlResolverCatalog::compose(&catalog, sources, &served).unwrap();
        assert!(resolvers.entities.is_empty());
        assert!(
            resolvers
                .relations
                .iter()
                .all(|relation| relation.type_name != "Parent" && relation.target != "Parent")
        );
        // `Summary` has no source at all and is still a target.
        assert!(resolvers.relation("Child", "Summary").is_some());
        assert_eq!(resolvers.relations.len(), 1);
        resolvers.validate_against(&catalog).unwrap();
    }

    /// A key may name as many fields as an entity can have, and a document
    /// over a bound is refused with an error, never a panic.
    #[test]
    fn wide_keys_compose_and_bounds_fail_closed() {
        let names = (0..17).map(|index| format!("K{index}")).collect::<Vec<_>>();
        let catalog = GraphqlSemanticCatalog::compose_descriptors(
            [entity(
                "Wide",
                names
                    .iter()
                    .map(|name| field(name, scalar("String")))
                    .collect(),
            )],
            [],
        )
        .unwrap();
        let source =
            GraphqlGeneratedResolverSource::new("Wide").with_federation_keys(vec![names.clone()]);
        let resolvers = compose(&catalog, [source]).unwrap();
        assert_eq!(resolvers.entity("Wide").unwrap().keys[0].fields, names);
        resolvers.validate_against(&catalog).unwrap();

        let too_many = (0..=GRAPHQL_RESOLVER_MAXIMUM_KEY_FIELDS)
            .map(|index| format!("K{index}"))
            .collect::<Vec<_>>();
        let source = GraphqlGeneratedResolverSource::new("Wide")
            .with_federation_keys(vec![too_many.clone()]);
        let error = compose(&catalog, [source]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "resolver catalogue entity resolver is invalid"
        );
        let mut edited = resolvers.clone();
        edited.entities[0].keys[0].fields = too_many;
        assert!(resealed(edited).validate().is_err());
        let mut edited = resolvers;
        edited.entities[0].keys = (0..=GRAPHQL_RESOLVER_MAXIMUM_KEY_FIELDS)
            .map(|index| GraphqlEntityResolverKey::new([format!("K{index}")]))
            .collect();
        assert!(resealed(edited).validate().is_err());
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
