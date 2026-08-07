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
