use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[serde(rename_all = "camelCase")]
#[graphql_entity(
    table = "collections",
    plural = "Collections",
    default_sort = "name ASC"
)]
struct Collection {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "uuid")]
    #[sortable]
    pub cover_stored_file_id: Option<graphql_orm::uuid::Uuid>,

    #[filterable(type = "string")]
    pub description: Option<String>,

    #[filterable(type = "number")]
    pub published_at: Option<i64>,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "clear_triggers",
    plural = "ClearTriggers",
    default_sort = "state ASC"
)]
struct ClearTrigger {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "uuid")]
    pub collection_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    pub state: String,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [Collection, ClearTrigger],
}

#[derive(Clone, Default)]
struct ClearNullableFieldsHook;

impl graphql_orm::graphql::orm::MutationHook for ClearNullableFieldsHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.phase != graphql_orm::graphql::orm::MutationPhase::After
                || event.action != graphql_orm::graphql::orm::ChangeAction::Updated
                || event.entity_name != "ClearTrigger"
            {
                return Ok(());
            }

            let trigger = event
                .after::<ClearTrigger>()?
                .ok_or_else(|| async_graphql::Error::new("missing updated trigger"))?;
            if trigger.state != "clear" {
                return Ok(());
            }

            hook_ctx
                .update_by_id::<Collection>(
                    &trigger.collection_id,
                    UpdateCollectionInput {
                        cover_stored_file_id: Some(None),
                        description: Some(None),
                        published_at: Some(None),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| async_graphql::Error::new(error.to_string()))?;

            Ok(())
        })
    }
}

#[cfg(feature = "sqlite")]
type TestPool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type TestPool = sqlx::PgPool;

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE collections (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            cover_stored_file_id TEXT NULL,
            description TEXT NULL,
            published_at INTEGER NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE clear_triggers (
            id TEXT PRIMARY KEY,
            collection_id TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&database_url).await?;
    sqlx::query("DROP TABLE IF EXISTS clear_triggers")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS collections")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE collections (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL,
            cover_stored_file_id UUID NULL,
            description TEXT NULL,
            published_at BIGINT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE clear_triggers (
            id UUID PRIMARY KEY,
            collection_id UUID NOT NULL,
            state TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[tokio::test]
async fn graphql_updates_distinguish_omitted_set_and_clear_for_nullable_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), ClearNullableFieldsHook);
    let schema = schema_builder(db.clone())
        .data("test-user".to_string())
        .finish();

    let initial_cover_id = graphql_orm::uuid::Uuid::new_v4();
    let replacement_cover_id = graphql_orm::uuid::Uuid::new_v4();
    let initial_cover_id_str = initial_cover_id.to_string();
    let replacement_cover_id_str = replacement_cover_id.to_string();

    let created = Collection::insert(
        &db,
        CreateCollectionInput {
            name: "Alpha".to_string(),
            cover_stored_file_id: Some(initial_cover_id),
            description: Some("Original description".to_string()),
            published_at: Some(123),
        },
    )
    .await?;

    let omitted = schema
        .execute(format!(
            "mutation {{
                updateCollection(id: \"{}\", input: {{ name: \"Renamed\" }}) {{
                    success
                    collection {{
                        id
                        name
                        coverStoredFileId
                        description
                        publishedAt
                    }}
                }}
            }}",
            created.id
        ))
        .await;
    assert!(omitted.errors.is_empty(), "{:?}", omitted.errors);
    let omitted_json = omitted.data.into_json()?;
    assert_eq!(
        omitted_json["updateCollection"]["collection"]["name"].as_str(),
        Some("Renamed")
    );
    assert_eq!(
        omitted_json["updateCollection"]["collection"]["coverStoredFileId"].as_str(),
        Some(initial_cover_id_str.as_str())
    );
    assert_eq!(
        omitted_json["updateCollection"]["collection"]["description"].as_str(),
        Some("Original description")
    );
    assert_eq!(
        omitted_json["updateCollection"]["collection"]["publishedAt"].as_i64(),
        Some(123)
    );

    let set = schema
        .execute(format!(
            "mutation {{
                updateCollection(
                    id: \"{}\"
                    input: {{
                        coverStoredFileId: \"{}\"
                        description: \"Updated description\"
                        publishedAt: 456
                    }}
                ) {{
                    success
                    collection {{
                        coverStoredFileId
                        description
                        publishedAt
                    }}
                }}
            }}",
            created.id, replacement_cover_id
        ))
        .await;
    assert!(set.errors.is_empty(), "{:?}", set.errors);
    let set_json = set.data.into_json()?;
    assert_eq!(
        set_json["updateCollection"]["collection"]["coverStoredFileId"].as_str(),
        Some(replacement_cover_id_str.as_str())
    );
    assert_eq!(
        set_json["updateCollection"]["collection"]["description"].as_str(),
        Some("Updated description")
    );
    assert_eq!(
        set_json["updateCollection"]["collection"]["publishedAt"].as_i64(),
        Some(456)
    );

    let cleared = schema
        .execute(format!(
            "mutation {{
                updateCollection(
                    id: \"{}\"
                    input: {{
                        coverStoredFileId: null
                        description: null
                        publishedAt: null
                    }}
                ) {{
                    success
                    collection {{
                        coverStoredFileId
                        description
                        publishedAt
                    }}
                }}
            }}",
            created.id
        ))
        .await;
    assert!(cleared.errors.is_empty(), "{:?}", cleared.errors);
    let cleared_json = cleared.data.into_json()?;
    assert!(
        cleared_json["updateCollection"]["success"]
            .as_bool()
            .unwrap_or(false)
    );
    assert!(cleared_json["updateCollection"]["collection"]["coverStoredFileId"].is_null());
    assert!(cleared_json["updateCollection"]["collection"]["description"].is_null());
    assert!(cleared_json["updateCollection"]["collection"]["publishedAt"].is_null());

    let fetched = Collection::get(&pool, &created.id)
        .await?
        .expect("collection should still exist");
    assert_eq!(fetched.cover_stored_file_id, None);
    assert_eq!(fetched.description, None);
    assert_eq!(fetched.published_at, None);

    let restored = Collection::update_by_id(
        &db,
        &created.id,
        UpdateCollectionInput {
            cover_stored_file_id: Some(Some(initial_cover_id)),
            description: Some(Some("Restored".to_string())),
            published_at: Some(Some(789)),
            ..Default::default()
        },
    )
    .await?
    .expect("collection should update");
    assert_eq!(restored.cover_stored_file_id, Some(initial_cover_id));
    assert_eq!(restored.description.as_deref(), Some("Restored"));
    assert_eq!(restored.published_at, Some(789));

    let trigger = ClearTrigger::insert(
        &db,
        CreateClearTriggerInput {
            collection_id: created.id,
            state: "idle".to_string(),
        },
    )
    .await?;
    ClearTrigger::update_by_id(
        &db,
        &trigger.id,
        UpdateClearTriggerInput {
            state: Some("clear".to_string()),
            ..Default::default()
        },
    )
    .await?
    .expect("trigger should update");

    let hook_cleared = Collection::get(&pool, &created.id)
        .await?
        .expect("collection should exist after hook");
    assert_eq!(hook_cleared.cover_stored_file_id, None);
    assert_eq!(hook_cleared.description, None);
    assert_eq!(hook_cleared.published_at, None);

    Ok(())
}
