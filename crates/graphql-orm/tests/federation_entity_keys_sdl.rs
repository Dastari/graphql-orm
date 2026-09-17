#![cfg(feature = "sqlite")]
//! Exported `@key` directives for entities opted into Federation.
//!
//! async-graphql builds the key string from the entity resolver's argument
//! names, so these tests assert the exact directive text rather than merely
//! that a key exists. The duplicate guard protects against a second
//! `registry.add_keys` call for the same field set.

use graphql_orm::async_graphql::SDLExportOptions;
use graphql_orm::prelude::*;

fn type_directives<'a>(sdl: &'a str, type_name: &str) -> &'a str {
    let needle = format!("type {type_name} ");
    let start = sdl
        .find(&needle)
        .or_else(|| sdl.find(&format!("type {type_name}\n")))
        .unwrap_or_else(|| panic!("federation SDL declares {type_name}:\n{sdl}"));
    let rest = &sdl[start..];
    let end = rest.find('{').expect("object has a field block");
    rest[..end].trim_end()
}

mod single_key {
    use super::*;

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "zones",
        plural = "Zones",
        default_sort = "name ASC",
        federation_key
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

    pub async fn sdl() -> String {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("SQLite test pool");
        schema_builder(graphql_orm::db::Database::new(pool))
            .finish()
            .sdl_with_options(SDLExportOptions::new().federation())
    }
}

mod composite_key {
    use super::*;

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
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

    pub async fn sdl() -> String {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("SQLite test pool");
        schema_builder(graphql_orm::db::Database::new(pool))
            .finish()
            .sdl_with_options(SDLExportOptions::new().federation())
    }
}

mod multiple_keys {
    use super::*;

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "assets",
        plural = "Assets",
        default_sort = "id ASC",
        federation_key,
        federation_key(fields = ["serial"])
    )]
    pub struct Asset {
        #[primary_key]
        pub id: String,

        #[unique]
        #[filterable(type = "string")]
        #[sortable]
        pub serial: String,

        pub label: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [Asset],
    }

    pub async fn sdl() -> String {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("SQLite test pool");
        schema_builder(graphql_orm::db::Database::new(pool))
            .finish()
            .sdl_with_options(SDLExportOptions::new().federation())
    }
}

mod unique_index_key {
    use super::*;

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "tenancies",
        plural = "Tenancies",
        default_sort = "id ASC",
        federation_key(fields = ["tenant", "code"]),
        unique_index(name = "tenancies_tenant_code", columns = ["tenant", "code"])
    )]
    pub struct Tenancy {
        #[primary_key]
        pub id: String,

        #[filterable(type = "string")]
        #[sortable]
        pub tenant: String,

        #[filterable(type = "string")]
        pub code: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [Tenancy],
    }

    pub async fn sdl() -> String {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("SQLite test pool");
        schema_builder(graphql_orm::db::Database::new(pool))
            .finish()
            .sdl_with_options(SDLExportOptions::new().federation())
    }
}

#[tokio::test]
async fn primary_key_entity_exports_one_resolvable_key() {
    let sdl = single_key::sdl().await;
    assert_eq!(
        type_directives(&sdl, "Zone"),
        "type Zone @key(fields: \"id\")"
    );
    assert_eq!(
        sdl.matches("@key(fields: \"id\")").count(),
        1,
        "a key is registered exactly once:\n{sdl}"
    );
}

#[tokio::test]
async fn composite_primary_key_exports_a_space_separated_key() {
    let sdl = composite_key::sdl().await;
    assert_eq!(
        type_directives(&sdl, "AssetPlacement"),
        "type AssetPlacement @key(fields: \"zoneId slot\")"
    );
}

#[tokio::test]
async fn two_declarations_export_two_distinct_keys() {
    let sdl = multiple_keys::sdl().await;
    let directives = type_directives(&sdl, "Asset");
    assert!(
        directives.contains("@key(fields: \"id\")"),
        "primary key is exported: {directives}"
    );
    assert!(
        directives.contains("@key(fields: \"serial\")"),
        "unique alternate key is exported: {directives}"
    );
    assert_eq!(
        directives.matches("@key(").count(),
        2,
        "exactly two keys are registered: {directives}"
    );
}

#[tokio::test]
async fn declared_unique_index_supports_a_non_primary_key() {
    let sdl = unique_index_key::sdl().await;
    assert_eq!(
        type_directives(&sdl, "Tenancy"),
        "type Tenancy @key(fields: \"tenant code\")"
    );
}

#[tokio::test]
async fn federation_export_omits_the_entity_service_fields() {
    let sdl = single_key::sdl().await;
    for absent in ["_entities", "_service", "union _Entity", "scalar _Any"] {
        assert!(
            !sdl.contains(absent),
            "federation SDL export omits {absent}:\n{sdl}"
        );
    }
}

#[tokio::test]
async fn a_resolvable_key_adds_the_entity_fields_to_introspection() {
    // Declaring any resolvable key enables federation on the schema, which adds
    // `_entities` and `_service` to the introspected schema even though the
    // federation SDL export hides them. Consumers that snapshot the plain SDL
    // see this change the moment they adopt a key.
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("SQLite test pool");
    let sdl = single_key::schema_builder(graphql_orm::db::Database::new(pool))
        .finish()
        .sdl();
    assert!(
        sdl.contains("_entities(representations: [_Any!]!): [_Entity]!"),
        "a keyed subgraph introspects the entity resolution field:\n{sdl}"
    );
    assert!(
        sdl.contains("_service: _Service!"),
        "a keyed subgraph introspects the service field:\n{sdl}"
    );
}
