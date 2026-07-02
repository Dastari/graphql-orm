#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;

mod none_mode {
    use graphql_orm::prelude::*;

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
        table = "mutation_exposure_hidden_notes",
        plural = "HiddenNotes",
        default_sort = "title ASC"
    )]
    pub struct HiddenNote {
        #[primary_key]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,

        #[filterable(type = "string")]
        #[sortable]
        pub title: String,
    }

    #[derive(Default)]
    pub struct ManualMutations;

    #[graphql_orm::async_graphql::Object]
    impl ManualMutations {
        async fn manual_ping(&self) -> bool {
            true
        }
    }

    schema_roots! {
        backend: "sqlite",
        generated_mutations: "none",
        entities: [HiddenNote],
        extra_mutation_types: [ManualMutations],
    }

    pub async fn create_table(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "CREATE TABLE mutation_exposure_hidden_notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL
            )",
        )
        .execute(pool)
        .await?;
        Ok(())
    }
}

mod allowlist_mode {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "mutation_exposure_allowlisted_notes",
        plural = "AllowlistedNotes"
    )]
    pub struct AllowlistedNote {
        #[primary_key]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,

        #[filterable(type = "string")]
        #[sortable]
        pub title: String,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "mutation_exposure_blocked_notes",
        plural = "BlockedNotes"
    )]
    pub struct BlockedNote {
        #[primary_key]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,

        #[filterable(type = "string")]
        #[sortable]
        pub title: String,
    }

    schema_roots! {
        backend: "sqlite",
        generated_mutations: "allowlist",
        generated_mutation_allowlist: [AllowlistedNote],
        entities: [AllowlistedNote, BlockedNote],
    }
}

mod denylist_mode {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "mutation_exposure_visible_notes",
        plural = "VisibleNotes"
    )]
    pub struct VisibleNote {
        #[primary_key]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,

        #[filterable(type = "string")]
        #[sortable]
        pub title: String,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "mutation_exposure_denied_notes",
        plural = "DeniedNotes"
    )]
    pub struct DeniedNote {
        #[primary_key]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,

        #[filterable(type = "string")]
        #[sortable]
        pub title: String,
    }

    schema_roots! {
        backend: "sqlite",
        generated_mutations: "denylist",
        generated_mutation_denylist: [DeniedNote],
        entities: [VisibleNote, DeniedNote],
    }
}

async fn sqlite_database() -> Result<Database<SqliteBackend>, Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    Ok(Database::new(pool))
}

#[tokio::test]
async fn generated_mutations_none_hides_public_root_but_keeps_repository_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    none_mode::create_table(&pool).await?;
    let database = Database::new(pool);
    let schema = none_mode::schema_builder(database.clone()).finish();
    let sdl = schema.sdl();

    assert!(sdl.contains("manualPing"));
    assert!(!sdl.contains("createHiddenNote"));
    assert!(!sdl.contains("updateHiddenNote"));
    assert!(!sdl.contains("deleteHiddenNote"));

    let created = none_mode::HiddenNote::insert(
        &database,
        none_mode::CreateHiddenNoteInput {
            title: "repository write".to_string(),
        },
    )
    .await?;
    assert_eq!(created.title, "repository write");
    Ok(())
}

#[tokio::test]
async fn generated_mutations_allowlist_exposes_only_listed_entities()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = allowlist_mode::schema_builder(sqlite_database().await?).finish();
    let sdl = schema.sdl();

    assert!(sdl.contains("createAllowlistedNote"));
    assert!(sdl.contains("updateAllowlistedNote"));
    assert!(sdl.contains("deleteAllowlistedNote"));
    assert!(!sdl.contains("createBlockedNote"));
    assert!(!sdl.contains("updateBlockedNote"));
    assert!(!sdl.contains("deleteBlockedNote"));
    Ok(())
}

#[tokio::test]
async fn generated_mutations_denylist_excludes_only_listed_entities()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = denylist_mode::schema_builder(sqlite_database().await?).finish();
    let sdl = schema.sdl();

    assert!(sdl.contains("createVisibleNote"));
    assert!(sdl.contains("updateVisibleNote"));
    assert!(sdl.contains("deleteVisibleNote"));
    assert!(!sdl.contains("createDeniedNote"));
    assert!(!sdl.contains("updateDeniedNote"));
    assert!(!sdl.contains("deleteDeniedNote"));
    Ok(())
}
