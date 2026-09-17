#![cfg(feature = "postgres")]
//! Federation keys under the PostgreSQL backend.
//!
//! The key predicate is rendered by the shared dialect-neutral filter path, so
//! this lane proves the codegen compiles and exports the same `@key` with
//! PostgreSQL placeholders and identifier quoting selected. It needs no
//! database: SDL export is a pure function of the generated types.

use graphql_orm::async_graphql::{EmptyMutation, EmptySubscription, SDLExportOptions, Schema};
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "federation_zones",
    plural = "Zones",
    default_sort = "name ASC",
    federation_key,
    federation_key(fields = ["tenant", "code"]),
    unique_index(name = "federation_zones_tenant_code", columns = ["tenant", "code"])
)]
struct Zone {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "string")]
    pub tenant: String,

    #[filterable(type = "string")]
    pub code: String,
}

schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [Zone],
}

#[test]
fn postgres_entities_export_single_and_composite_keys() {
    let schema = Schema::build(QueryRoot::default(), EmptyMutation, EmptySubscription).finish();
    let sdl = schema.sdl_with_options(SDLExportOptions::new().federation());

    assert!(
        sdl.contains("@key(fields: \"id\")"),
        "the primary key is exported:\n{sdl}"
    );
    assert!(
        sdl.contains("@key(fields: \"tenant code\")"),
        "the composite unique key is exported:\n{sdl}"
    );
    assert!(
        sdl.contains("type PageInfo @shareable"),
        "PageInfo stays shareable on every backend:\n{sdl}"
    );
}
