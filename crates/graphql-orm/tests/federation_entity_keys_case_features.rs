#![cfg(all(
    feature = "sqlite",
    feature = "field-case-pascal",
    feature = "argument-case-snake"
))]
//! The `@key` string must follow field case, not argument case.
//!
//! async-graphql builds the key from the entity resolver's argument names, and
//! the ORM configures argument case independently of field case. Under this
//! lane the generated single read takes `zone_id`/`slot` while the entity
//! exports `ZoneId`/`Slot`, so a resolver that accepted the default argument
//! names would export a `@key` naming fields that do not exist.

use graphql_orm::async_graphql::SDLExportOptions;
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "asset_placements",
    plural = "AssetPlacements",
    default_sort = "zone_id ASC",
    federation_key
)]
pub struct AssetPlacement {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub zone_id: String,

    #[primary_key]
    pub slot: i32,

    #[filterable(type = "string")]
    pub label: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [AssetPlacement],
}

#[tokio::test]
async fn the_key_names_exported_fields_not_generated_arguments() {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    let schema = schema_builder(graphql_orm::db::Database::new(pool)).finish();
    let federation_sdl = schema.sdl_with_options(SDLExportOptions::new().federation());

    assert!(
        federation_sdl.contains("type AssetPlacement @key(fields: \"ZoneId Slot\")"),
        "the key follows field case:\n{federation_sdl}"
    );
    assert!(
        !federation_sdl.contains("@key(fields: \"zone_id slot\")"),
        "the key must not follow argument case:\n{federation_sdl}"
    );

    // The single read still uses argument case, which is exactly the mismatch
    // the renamed key arguments protect against.
    let sdl = schema.sdl();
    assert!(
        sdl.contains("zone_id: String!") && sdl.contains("slot: Int!"),
        "the generated single read keeps argument case:\n{sdl}"
    );
    assert!(
        sdl.contains("ZoneId: String!"),
        "the entity exports PascalCase fields:\n{sdl}"
    );
}
