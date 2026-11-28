//! Resolver catalogue of one generated schema root.
//!
//! `schema_roots!` emits `graphql_orm_resolver_catalog()`, which composes
//! [`GraphqlResolverCatalog`] with [`compose_resolver_catalog`]: the
//! relationship fields and the resolvable Federation keys the schema serves
//! (#544). The document is descriptive, grants no authority, and changes no
//! other catalogue or fingerprint.
//!
//! The served schema decides what is listed. The query root's type registry
//! gives each entity's registered type name, its fields with their types and
//! arguments, and its resolvable keys. A relation whose resolver is not
//! mounted, a field that only shares a relation's name, and a key the schema
//! does not resolve have no entry.

use async_graphql::OutputType;
use async_graphql::registry::{MetaType, Registry};

use super::{
    Entity, GraphqlGeneratedResolverSource, GraphqlHandwrittenResolverRegistration,
    GraphqlResolverCatalog, GraphqlSemanticCatalog, GraphqlSemanticEntitySource,
    GraphqlSemanticError, GraphqlServedField, GraphqlServedObjects,
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
    rust_type: &'static str,
    mounted_relations: &'static [&'static str],
    facts: GeneratedResolverFacts,
}

impl ResolverEntitySource {
    /// Captures an entity's generated resolver facts.
    ///
    /// The entity's Rust type identifies its object in the schema's type
    /// registry. `mounted_relations` is the entity's generated-output witness: the
    /// relation fields whose generated resolver its GraphQL object mounts. It
    /// is empty for an entity with no generated relation resolver.
    pub fn of<T: Entity>(
        facts: GeneratedResolverFacts,
        mounted_relations: &'static [&'static str],
    ) -> Self {
        Self {
            semantic_name: T::graphql_semantic_metadata().map(|entity| entity.entity_name.clone()),
            rust_type: std::any::type_name::<T>(),
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
    compose_resolver_catalog_with_registrations::<Q>(catalog, sources, &[], [])
}

/// Composes the resolver catalogue for `Q` with handwritten field registrations.
///
/// # Errors
///
/// Returns an error when the generated metadata or any registration fails
/// validation.
#[doc(hidden)]
pub fn compose_resolver_catalog_with_registrations<Q: OutputType>(
    catalog: &GraphqlSemanticCatalog,
    sources: Vec<ResolverEntitySource>,
    custom_sources: &[GraphqlSemanticEntitySource],
    registrations: impl IntoIterator<Item = GraphqlHandwrittenResolverRegistration>,
) -> Result<GraphqlResolverCatalog, GraphqlSemanticError> {
    let mut registry = Registry::default();
    Q::create_type_info(&mut registry);
    let mut served = GraphqlServedObjects::new();
    for (type_name, registered) in &registry.types {
        if let MetaType::Object { fields, .. } = registered {
            served = served.with_object(
                type_name,
                fields.values().map(|field| {
                    field.args.values().fold(
                        GraphqlServedField::new(&field.name, &field.ty),
                        |served, argument| served.with_argument(&argument.name, &argument.ty),
                    )
                }),
            );
        }
    }
    let matching_objects = |rust_type| {
        registry
            .types
            .iter()
            .filter_map(move |(type_name, registered)| match registered {
                MetaType::Object {
                    rust_typename: Some(registered_rust_type),
                    keys,
                    resolvable,
                    ..
                } if *registered_rust_type == rust_type => Some((
                    type_name,
                    keys.as_deref().filter(|_| *resolvable).unwrap_or_default(),
                )),
                _ => None,
            })
    };
    let mut registrations = registrations.into_iter().peekable();
    if registrations.peek().is_some() {
        let entity_types = sources
            .iter()
            .filter_map(|source| Some((source.semantic_name.as_deref()?, source.rust_type)));
        let custom_types = custom_sources.iter().filter_map(|source| {
            Some((
                source.metadata().entity_name.as_str(),
                source.rust_type_name()?,
            ))
        });
        served = served.with_registration_type_names(
            entity_types
                .chain(custom_types)
                .filter_map(|(semantic_name, rust_type)| {
                    let mut objects = matching_objects(rust_type);
                    let (served_name, _) = objects.next()?;
                    objects
                        .next()
                        .is_none()
                        .then(|| (semantic_name.to_owned(), served_name.clone()))
                })
                .collect(),
        );
    }
    let sources = sources.into_iter().filter_map(|source| {
        let entity_name = source.semantic_name?;
        // The object the schema registers for this Rust type gives the
        // served type name and the keys the schema resolves for it, as it
        // spells them in `@key(fields: "...")`. A type the subgraph marks
        // unresolvable resolves none of its keys.
        let registered = matching_objects(source.rust_type).next();
        // An entity the schema does not register is kept as such, so that
        // nothing of it is matched to another type with its name.
        let Some((type_name, resolvable)) = registered else {
            return Some(GraphqlGeneratedResolverSource::new(entity_name).unregistered());
        };
        let owned = |name: Option<&str>| name.map(str::to_owned);
        Some(
            GraphqlGeneratedResolverSource::new(entity_name)
                .with_type_name(type_name)
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
    GraphqlResolverCatalog::compose_with_registrations(catalog, sources, &served, registrations)
}
