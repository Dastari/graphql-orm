#![cfg(feature = "mssql")]
//! Federation keys on an externally owned SQL Server entity.
//!
//! The key predicate is rendered by the shared dialect-neutral filter path, so
//! this lane proves the codegen compiles and exports the same `@key` under a
//! read-only external schema whose unique constraint the ORM does not declare.

use graphql_orm::async_graphql::{EmptyMutation, EmptySubscription, SDLExportOptions, Schema};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.FederationZones",
    plural = "ExternalZones",
    schema_policy = "external_read_only",
    default_sort = "[ZoneId] ASC",
    federation_key,
    federation_key(fields = ["externalRef"], assume_unique = true)
)]
struct ExternalZone {
    #[primary_key]
    #[graphql_orm(db_column = "ZoneId", write = false)]
    #[sortable]
    pub id: i64,

    #[graphql_orm(db_column = "ExternalRef", write = false)]
    #[filterable(type = "string")]
    pub external_ref: String,

    #[graphql_orm(db_column = "ZoneName", write = false)]
    #[filterable(type = "string")]
    pub name: String,
}

schema_roots! {
    backend: "mssql",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    entities: [ExternalZone],
}

#[test]
fn an_external_read_only_entity_exports_both_declared_keys() {
    let schema = Schema::build(QueryRoot::default(), EmptyMutation, EmptySubscription).finish();
    let sdl = schema.sdl_with_options(SDLExportOptions::new().federation());

    assert!(
        sdl.contains("@key(fields: \"id\")"),
        "the primary key is exported:\n{sdl}"
    );
    assert!(
        sdl.contains("@key(fields: \"externalRef\")"),
        "the assumed-unique key is exported:\n{sdl}"
    );
    assert!(
        sdl.contains("type PageInfo @shareable"),
        "PageInfo stays shareable on every backend:\n{sdl}"
    );
}
