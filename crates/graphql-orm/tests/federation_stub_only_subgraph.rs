#![cfg(feature = "sqlite")]
//! A subgraph that only references foreign entities must opt into federation.
//!
//! async-graphql enables federation automatically as soon as one resolvable
//! `@key` exists. A subgraph that owns no entity and only declares
//! `unresolvable` reference stubs has no such key, so it needs
//! `federation: true` on `schema_roots!` to serve `_service` and to export the
//! federation SDL that composition reads.

use graphql_orm::async_graphql::SDLExportOptions;
use graphql_orm::prelude::*;

/// A reference to an entity owned by another subgraph.
///
/// Reference stubs stay plain async-graphql: the ORM has no table for them, so
/// there is nothing for it to generate or validate.
#[derive(graphql_orm::async_graphql::SimpleObject, Clone, Debug)]
#[graphql(name = "Zone", unresolvable = "id")]
pub struct ZoneReference {
    pub id: String,
}

#[derive(Default)]
pub struct ReadingLinks;

#[graphql_orm::async_graphql::Object]
impl ReadingLinks {
    /// The owning subgraph resolves this reference through `_entities`.
    #[graphql(name = "readingZone")]
    async fn reading_zone(&self, id: String) -> ZoneReference {
        ZoneReference { id }
    }
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "readings",
    plural = "Readings",
    default_sort = "id ASC"
)]
pub struct Reading {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub zone_id: String,
}

schema_roots! {
    backend: "sqlite",
    federation: true,
    query_custom_ops: [],
    entities: [Reading],
    extra_query_types: [ReadingLinks],
}

#[tokio::test]
async fn a_stub_only_subgraph_opts_into_federation() {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    let schema = schema_builder(graphql_orm::db::Database::new(pool)).finish();

    let federation_sdl = schema.sdl_with_options(SDLExportOptions::new().federation());
    assert!(
        federation_sdl.contains("type Zone @key(fields: \"id\", resolvable: false)"),
        "the stub declares an unresolvable key:\n{federation_sdl}"
    );

    let response = schema.execute("{ _service { sdl } }").await;
    assert!(
        response.errors.is_empty(),
        "federation is enabled without a resolvable key: {:?}",
        response.errors
    );
}
