//! Resolver catalogue of one generated schema root.
//!
//! `schema_roots!` emits `graphql_orm_resolver_catalog()`, which composes
//! [`GraphqlResolverCatalog`] with [`compose_resolver_catalog`]: every
//! relationship field of the semantic catalogue and every entity with a
//! resolvable Federation key (#544). The document is descriptive, grants no
//! authority, and changes no other catalogue or fingerprint.

use super::{
    Entity, GraphqlGeneratedResolverSource, GraphqlResolverCatalog, GraphqlSemanticCatalog,
};

/// Compile-time resolver facts of one entity, emitted by `GraphQLOperations`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct GeneratedResolverFacts {
    /// Resolvable `@key` field sets, in declaration order.
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
pub struct ResolverEntitySource(Option<GraphqlGeneratedResolverSource>);

impl ResolverEntitySource {
    /// Captures an entity's generated resolver facts.
    pub fn of<T: Entity>(facts: GeneratedResolverFacts) -> Self {
        let owned = |name: Option<&str>| name.map(str::to_owned);
        Self(T::graphql_semantic_metadata().map(|semantic| {
            GraphqlGeneratedResolverSource::new(semantic.entity_name.clone())
                .with_relation_fields(
                    T::relation_metadata()
                        .iter()
                        .map(|relation| relation.field_name.to_owned())
                        .collect(),
                )
                .with_federation_keys(
                    facts
                        .federation_keys
                        .iter()
                        .map(|key| key.iter().map(|field| (*field).to_owned()).collect())
                        .collect(),
                )
                .with_traversal_roots(owned(facts.to_many_root), owned(facts.to_one_root))
                .with_entity_root(owned(facts.entity_root))
        }))
    }
}

/// Composes the resolver catalogue for one generated schema root.
///
/// # Panics
///
/// Panics when the composed document does not validate against the
/// catalogue, which is a generator bug.
#[doc(hidden)]
pub fn compose_resolver_catalog(
    catalog: &GraphqlSemanticCatalog,
    sources: Vec<ResolverEntitySource>,
) -> GraphqlResolverCatalog {
    GraphqlResolverCatalog::compose(catalog, sources.into_iter().filter_map(|source| source.0))
        .expect("macro-generated GraphQL resolver catalogue must validate")
}
