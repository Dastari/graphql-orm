#![cfg(feature = "sqlite")]
//! Every generated subgraph exports the same `PageInfo` object.
//!
//! Federation v2 composition rejects a supergraph in which two subgraphs define
//! the same non-entity object field without `@shareable`. Two `graphql-orm`
//! subgraphs always collide on `PageInfo`, so the exported type must carry the
//! directive. The full composition proof lives in the excluded end-to-end
//! federation fixture; this test pins the exported SDL that composition reads.

use graphql_orm::async_graphql::SDLExportOptions;
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "shareable_zones",
    plural = "Zones",
    default_sort = "name ASC"
)]
pub struct Zone {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [Zone],
}

async fn federation_sdl() -> String {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    schema_builder(graphql_orm::db::Database::new(pool))
        .finish()
        .sdl_with_options(SDLExportOptions::new().federation())
}

fn page_info_definition(sdl: &str) -> &str {
    let start = sdl
        .find("type PageInfo")
        .expect("federation SDL declares PageInfo");
    let rest = &sdl[start..];
    let end = rest.find('{').expect("PageInfo has a field block");
    rest[..end].trim_end()
}

#[tokio::test]
async fn exported_page_info_is_shareable() {
    let sdl = federation_sdl().await;
    let definition = page_info_definition(&sdl);
    assert_eq!(
        definition, "type PageInfo @shareable",
        "PageInfo must be @shareable so two graphql-orm subgraphs compose; got {definition:?}"
    );
}

#[tokio::test]
async fn shareable_is_declared_once_on_page_info() {
    let sdl = federation_sdl().await;
    assert_eq!(
        sdl.matches("type PageInfo").count(),
        1,
        "PageInfo is declared exactly once per subgraph"
    );
    assert_eq!(
        page_info_definition(&sdl).matches("@shareable").count(),
        1,
        "the shareable directive is applied exactly once"
    );
}
