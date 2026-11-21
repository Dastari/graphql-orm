//! Resolver catalogue of one generated schema root.
//!
//! `schema_roots!` emits `graphql_orm_resolver_catalog()`, which composes
//! [`GraphqlResolverCatalog`] with [`compose_resolver_catalog`]: the
//! relationship fields and the resolvable Federation keys the schema serves
//! (#544). The document is descriptive, grants no authority, and changes no
//! other catalogue or fingerprint.
//!
//! The served schema decides what is listed. The query root's type registry
//! gives each entity's registered type name, its fields and its resolvable
//! keys, so a relation whose resolver is not mounted, or a key the schema
//! does not resolve, has no entry.

use async_graphql::OutputType;
use async_graphql::registry::{MetaType, Registry};

use super::{
    Entity, GraphqlGeneratedResolverSource, GraphqlResolverCatalog, GraphqlSemanticCatalog,
    GraphqlSemanticError, GraphqlServedObjects,
};

/// Compile-time resolver facts of one entity, emitted by `GraphQLOperations`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct GeneratedResolverFacts {
    /// Declared resolvable `@key` field sets, in declaration order.
    pub federation_keys: &'static [&'static [&'static str]],
    /// The exposed list root, when a to-many traversal into the entity
    /// enforces exactly that root's static requirement.
    pub to_many_root: Option<&'static str>,
    /// The exposed single-read root, when a to-one traversal into the entity
    /// enforces exactly that root's static requirement.
    pub to_one_root: Option<&'static str>,
    /// The exposed single-read root, when the entity resolvers enforce
    /// exactly that root's static requirement.
    pub entity_root: Option<&'static str>,
}

/// One entity's input to [`compose_resolver_catalog`], captured by
/// `schema_roots!`.
#[doc(hidden)]
pub struct ResolverEntitySource {
    semantic_name: Option<String>,
    type_name: String,
    mounted_relations: &'static [&'static str],
    facts: GeneratedResolverFacts,
}

impl ResolverEntitySource {
    /// Captures an entity's generated resolver facts.
    ///
    /// `mounted_relations` is the entity's generated-output witness: the
    /// relation fields whose generated resolver its GraphQL object mounts. It
    /// is empty for an entity with no generated relation resolver.
    pub fn of<T: Entity + OutputType>(
        facts: GeneratedResolverFacts,
        mounted_relations: &'static [&'static str],
    ) -> Self {
        Self {
            semantic_name: T::graphql_semantic_metadata().map(|entity| entity.entity_name.clone()),
            type_name: <T as OutputType>::type_name().into_owned(),
            mounted_relations,
            facts,
        }
    }
}

/// Composes the resolver catalogue for one generated schema root, whose
/// query root is `Q`.
///
/// # Errors
///
/// Returns an error when the composed document does not validate against the
/// catalogue or exceeds one of its bounds.
#[doc(hidden)]
pub fn compose_resolver_catalog<Q: OutputType>(
    catalog: &GraphqlSemanticCatalog,
    sources: Vec<ResolverEntitySource>,
) -> Result<GraphqlResolverCatalog, GraphqlSemanticError> {
    let mut registry = Registry::default();
    Q::create_type_info(&mut registry);
    let mut served = GraphqlServedObjects::new();
    for (type_name, registered) in &registry.types {
        if let MetaType::Object { fields, .. } = registered {
            served = served.with_object(type_name, fields.keys());
        }
    }
    let sources = sources.into_iter().filter_map(|source| {
        let entity_name = source.semantic_name?;
        // The keys the served schema resolves for this type, as it spells
        // them in `@key(fields: "...")`.
        let resolvable = match registry.types.get(&source.type_name) {
            Some(MetaType::Object {
                keys: Some(keys), ..
            }) => keys.as_slice(),
            _ => &[],
        };
        let owned = |name: Option<&str>| name.map(str::to_owned);
        Some(
            GraphqlGeneratedResolverSource::new(entity_name)
                .with_type_name(source.type_name)
                .with_relation_fields(
                    source
                        .mounted_relations
                        .iter()
                        .map(|field| (*field).to_owned())
                        .collect(),
                )
                .with_federation_keys(
                    source
                        .facts
                        .federation_keys
                        .iter()
                        .filter(|key| resolvable.contains(&key.join(" ")))
                        .map(|key| key.iter().map(|field| (*field).to_owned()).collect())
                        .collect(),
                )
                .with_traversal_roots(
                    owned(source.facts.to_many_root),
                    owned(source.facts.to_one_root),
                )
                .with_entity_root(owned(source.facts.entity_root)),
        )
    });
    GraphqlResolverCatalog::compose(catalog, sources, &served)
}
