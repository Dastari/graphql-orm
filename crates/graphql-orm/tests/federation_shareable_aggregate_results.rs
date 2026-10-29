#![cfg(feature = "sqlite")]
//! Every aggregate-enabled subgraph exports the same aggregate result objects.
//!
//! Federation v2 composition rejects a supergraph in which two subgraphs define
//! the same non-entity object field without `@shareable`. Two `graphql-orm`
//! subgraphs that each opt one entity into `aggregate` always collide on
//! `GraphqlAggregateResultRow` and `GraphqlAggregateResultValue`, so the
//! exported types must carry the directive. The full composition proof lives in
//! the excluded end-to-end federation fixture; this test pins the exported SDL
//! that composition reads.

use graphql_orm::async_graphql::SDLExportOptions;
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "shareable_aggregate_meters",
    plural = "Meters",
    default_sort = "id ASC",
    aggregate = true,
    auth = "none"
)]
pub struct Meter {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub site: String,

    #[filterable(type = "number")]
    pub units: i64,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [Meter],
}

async fn federation_sdl() -> String {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    schema_builder(graphql_orm::db::Database::new(pool))
        .finish()
        .sdl_with_options(SDLExportOptions::new().federation())
}

fn type_header<'a>(sdl: &'a str, name: &str) -> &'a str {
    let needle = format!("type {name} ");
    let start = sdl
        .find(&needle)
        .unwrap_or_else(|| panic!("federation SDL declares {name}"));
    let rest = &sdl[start..];
    let end = rest
        .find('{')
        .unwrap_or_else(|| panic!("{name} has a field block"));
    rest[..end].trim_end()
}

#[tokio::test]
async fn exported_aggregate_result_objects_are_shareable() {
    let sdl = federation_sdl().await;
    for name in ["GraphqlAggregateResultRow", "GraphqlAggregateResultValue"] {
        let header = type_header(&sdl, name);
        assert_eq!(
            header,
            format!("type {name} @shareable"),
            "{name} must be @shareable so two aggregate-enabled graphql-orm subgraphs compose; got {header:?}"
        );
        assert_eq!(
            sdl.matches(&format!("type {name} ")).count(),
            1,
            "{name} is declared exactly once per subgraph"
        );
    }
}
