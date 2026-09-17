//! Federation SDL helpers for generated GraphQL types.

#![allow(missing_docs)]

/// Standard Federation `@authenticated`, addressed through the default
/// namespace of async-graphql's existing Federation v2.5 link.
///
/// async-graphql 7.2.1 does not import or model `@authenticated` directly. A
/// non-imported Federation directive remains available under the standard
/// `federation__` namespace, so generated fields use
/// `@federation__authenticated`. Federation composition recognizes that name
/// as the standard directive; this is not a project-owned composed directive.
#[async_graphql::TypeDirective(
    name = "federation__authenticated",
    location = "FieldDefinition",
    location = "Object",
    location = "Interface",
    location = "Enum"
)]
pub fn federation_authenticated() {}

/// Standard Federation `@requiresScopes` addressed through the existing
/// Federation link's default namespace for generated subscription fields.
///
/// async-graphql 7.2.1 exposes a dedicated `requires_scopes` attribute for
/// object fields but not subscription fields. The namespaced form preserves
/// the same standard Federation identity without defining a project-owned
/// authorization directive.
#[async_graphql::TypeDirective(name = "federation__requiresScopes", location = "FieldDefinition")]
pub fn federation_requires_scopes(scopes: Vec<Vec<String>>) {}

/// Witness that generated Federation entity resolvers exist for an entity.
///
/// `#[graphql_entity(federation_key)]` is read by two derives. `GraphQLEntity`
/// records the declaration, while `GraphQLOperations` is the derive that owns
/// the `{Entity}Queries` object where an `#[graphql(entity)]` resolver — the
/// only construct async-graphql turns into a resolvable `@key` — can be
/// emitted. Without this witness, declaring a key on a type that lacks
/// `GraphQLOperations` would silently export no key at all. `GraphQLEntity`
/// therefore requires this trait, and only the operations derive implements it.
#[diagnostic::on_unimplemented(
    message = "`{Self}` declares `federation_key` but generates no Federation entity resolver",
    label = "no entity resolver is generated for `{Self}`",
    note = "add `GraphQLOperations` to this type's derive list; only that derive emits the generated queries object whose entity resolver async-graphql turns into a resolvable `@key`"
)]
pub trait GeneratedFederationEntityKeys {
    /// Number of `@key` directives generated for this entity.
    const FEDERATION_KEY_COUNT: usize;
}
