#![cfg(feature = "sqlite")]

#[path = "support/federation_sdl.rs"]
mod federation_sdl;

use federation_sdl::ParsedFederationSchema;
use graphql_orm::async_graphql::SDLExportOptions;
use graphql_orm::prelude::*;

mod full_roots {
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
        table = "federation_ninja_devices",
        plural = "NinjaDevices",
        default_sort = "name ASC"
    )]
    pub struct NinjaDevice {
        #[primary_key]
        pub id: String,

        #[filterable(type = "string")]
        #[sortable]
        pub name: String,
    }

    schema_roots! {
        backend: "sqlite",
        query_custom_ops: [],
        entities: [NinjaDevice],
    }

    pub async fn federation_sdl() -> String {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("SQLite test pool");
        schema_builder(graphql_orm::db::Database::new(pool))
            .enable_subscription_in_federation()
            .finish()
            .sdl_with_options(SDLExportOptions::new().federation())
    }
}

mod zero_subscription {
    use super::*;

    #[derive(Default)]
    pub struct ProviderQueries;

    #[graphql_orm::async_graphql::Object]
    impl ProviderQueries {
        #[graphql(name = "NinjaDevices")]
        async fn ninja_devices(&self) -> Vec<String> {
            Vec::new()
        }
    }

    schema_roots! {
        backend: "sqlite",
        generated_mutations: "none",
        query_custom_ops: [],
        entities: [],
        extra_query_types: [ProviderQueries],
    }

    pub async fn federation_sdl() -> String {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("SQLite test pool");
        schema_builder(graphql_orm::db::Database::new(pool))
            .finish()
            .sdl_with_options(SDLExportOptions::new().federation())
    }
}

mod chunked_query {
    use super::*;

    #[rustfmt::skip]
    macro_rules! chunk_entity {
        ($name:ident, $table:literal, $plural:literal) => {
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
                schema_policy = "external_read_only",
                table = $table,
                plural = $plural,
                default_sort = "id ASC"
            )]
            pub struct $name {
                #[primary_key]
                pub id: String,

                #[sortable]
                pub label: String,
            }
        };
    }

    chunk_entity!(ChunkEntity01, "federation_chunk_01", "ChunkEntities01");
    chunk_entity!(ChunkEntity02, "federation_chunk_02", "ChunkEntities02");
    chunk_entity!(ChunkEntity03, "federation_chunk_03", "ChunkEntities03");
    chunk_entity!(ChunkEntity04, "federation_chunk_04", "ChunkEntities04");
    chunk_entity!(ChunkEntity05, "federation_chunk_05", "ChunkEntities05");
    chunk_entity!(ChunkEntity06, "federation_chunk_06", "ChunkEntities06");
    chunk_entity!(ChunkEntity07, "federation_chunk_07", "ChunkEntities07");
    chunk_entity!(ChunkEntity08, "federation_chunk_08", "ChunkEntities08");
    chunk_entity!(ChunkEntity09, "federation_chunk_09", "ChunkEntities09");
    chunk_entity!(ChunkEntity10, "federation_chunk_10", "ChunkEntities10");
    chunk_entity!(ChunkEntity11, "federation_chunk_11", "ChunkEntities11");
    chunk_entity!(ChunkEntity12, "federation_chunk_12", "ChunkEntities12");
    chunk_entity!(ChunkEntity13, "federation_chunk_13", "ChunkEntities13");

    schema_roots! {
        backend: "sqlite",
        schema_policy: "external_read_only",
        query_custom_ops: [],
        entities: [
            ChunkEntity01,
            ChunkEntity02,
            ChunkEntity03,
            ChunkEntity04,
            ChunkEntity05,
            ChunkEntity06,
            ChunkEntity07,
            ChunkEntity08,
            ChunkEntity09,
            ChunkEntity10,
            ChunkEntity11,
            ChunkEntity12,
            ChunkEntity13,
        ],
    }

    pub async fn federation_sdl() -> String {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("SQLite test pool");
        let database = graphql_orm::db::Database::builder(pool)
            .schema_policy(graphql_orm::graphql::orm::SchemaPolicy::ExternalReadOnly)
            .build();
        schema_builder(database)
            .finish()
            .sdl_with_options(SDLExportOptions::new().federation())
    }
}

#[tokio::test]
async fn generated_query_mutation_and_subscription_roots_are_federation_reachable() {
    let sdl = full_roots::federation_sdl().await;
    let parsed = ParsedFederationSchema::parse(&sdl);

    assert_eq!(parsed.query, "Query");
    assert_eq!(parsed.mutation.as_deref(), Some("Mutation"));
    assert_eq!(parsed.subscription.as_deref(), Some("Subscription"));
    assert!(parsed.query_fields().contains("ninjaDevices"));
    assert!(!parsed.objects.contains_key("QueryRoot"));
    assert!(!parsed.objects.contains_key("MutationRoot"));
    assert!(!parsed.objects.contains_key("SubscriptionRoot"));
}

#[tokio::test]
async fn zero_subscription_schema_has_no_dangling_or_fake_operation_roots() {
    let sdl = zero_subscription::federation_sdl().await;
    let parsed = ParsedFederationSchema::parse(&sdl);

    assert_eq!(parsed.query, "Query");
    assert!(parsed.query_fields().contains("NinjaDevices"));
    assert_eq!(parsed.mutation, None);
    assert_eq!(parsed.subscription, None);
    assert!(!parsed.objects.contains_key("Mutation"));
    assert!(!parsed.objects.contains_key("Subscription"));
    assert!(!sdl.contains("SubscriptionRoot"));
}

#[tokio::test]
async fn multiple_query_chunks_merge_every_entity_field_into_the_operation_root() {
    let _first_chunk = chunked_query::QueryRootChunk0::default();
    let _second_chunk = chunked_query::QueryRootChunk1::default();
    let sdl = chunked_query::federation_sdl().await;
    let parsed = ParsedFederationSchema::parse(&sdl);
    let query_fields = parsed.query_fields();

    for index in 1..=13 {
        assert!(
            query_fields.contains(&format!("chunkEntities{index:02}")),
            "chunked query field {index:02} must be reachable from Query"
        );
    }
    assert_eq!(parsed.mutation, None);
    assert_eq!(parsed.subscription, None);
}
