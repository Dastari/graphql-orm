use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "scoped_collections",
    plural = "ScopedCollections",
    default_sort = "name ASC"
)]
struct ScopedCollection {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[graphql_orm(private)]
    pub owner_user_id: String,

    #[graphql_orm(private)]
    pub updated_by_user_id: Option<String>,
}

schema_roots! {
    query_custom_ops: [],
    entities: [ScopedCollection],
}

#[derive(Clone, Default)]
struct ScopedRowPolicy;

impl graphql_orm::graphql::orm::RowPolicy for ScopedRowPolicy {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: graphql_orm::graphql::orm::EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if entity_name != "ScopedCollection" {
                return Ok(true);
            }
            let Some(collection) = row.downcast_ref::<ScopedCollection>() else {
                return Ok(false);
            };
            let actor = ctx.and_then(|ctx| ctx.data_opt::<String>()).cloned();
            Ok(matches!(actor.as_deref(), Some("admin"))
                || actor.as_deref() == Some(collection.owner_user_id.as_str()))
        })
    }

    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: graphql_orm::graphql::orm::EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if entity_name != "ScopedCollection" {
                return Ok(true);
            }
            let Some(collection) = row.downcast_ref::<ScopedCollection>() else {
                return Ok(false);
            };
            let actor = ctx.and_then(|ctx| ctx.data_opt::<String>()).cloned();
            Ok(matches!(actor.as_deref(), Some("admin"))
                || actor.as_deref() == Some(collection.owner_user_id.as_str()))
        })
    }
}

#[derive(Clone, Default)]
struct ScopedWriteTransform {
    audit: Arc<Mutex<Vec<String>>>,
}

impl ScopedWriteTransform {
    fn snapshot(&self) -> Vec<String> {
        self.audit.lock().expect("audit lock poisoned").clone()
    }
}

impl graphql_orm::graphql::orm::WriteInputTransform for ScopedWriteTransform {
    fn before_create<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if entity_name == "ScopedCollection" {
                let actor = ctx
                    .and_then(|ctx| ctx.data_opt::<String>())
                    .cloned()
                    .unwrap_or_else(|| "system".to_string());
                let input = input
                    .downcast_mut::<CreateScopedCollectionInput>()
                    .ok_or_else(|| async_graphql::Error::new("unexpected create input type"))?;
                input.owner_user_id = actor.clone();
                input.updated_by_user_id = Some(actor.clone());
                self.audit
                    .lock()
                    .expect("audit lock poisoned")
                    .push(format!("create:{actor}"));
            }
            Ok(())
        })
    }

    fn before_update<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        _existing_row: Option<&'a (dyn std::any::Any + Send + Sync)>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if entity_name == "ScopedCollection" {
                let actor = ctx
                    .and_then(|ctx| ctx.data_opt::<String>())
                    .cloned()
                    .unwrap_or_else(|| "system".to_string());
                let input = input
                    .downcast_mut::<UpdateScopedCollectionInput>()
                    .ok_or_else(|| async_graphql::Error::new("unexpected update input type"))?;
                input.updated_by_user_id = Some(Some(actor.clone()));
                self.audit
                    .lock()
                    .expect("audit lock poisoned")
                    .push(format!("update:{actor}"));
            }
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
        "CREATE TABLE scoped_collections (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            owner_user_id TEXT NOT NULL,
            updated_by_user_id TEXT
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
    sqlx::query("DROP TABLE IF EXISTS scoped_collections")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE scoped_collections (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL,
            owner_user_id TEXT NOT NULL,
            updated_by_user_id TEXT
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[tokio::test]
async fn row_policy_filters_reads_and_write_transform_injects_server_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let transform = ScopedWriteTransform::default();
    let mut db = graphql_orm::db::Database::new(pool.clone());
    db.set_row_policy(ScopedRowPolicy);
    db.set_write_input_transform(transform.clone());

    let actor_a_schema = schema_builder(db.clone())
        .data("actor-a".to_string())
        .finish();
    let actor_b_schema = schema_builder(db.clone())
        .data("actor-b".to_string())
        .finish();
    let admin_schema = schema_builder(db.clone())
        .data("admin".to_string())
        .finish();

    let created = actor_a_schema
        .execute(
            "mutation {
                createScopedCollection(input: { name: \"Alpha\" }) {
                    success
                    scopedCollection { id name }
                }
            }",
        )
        .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let created_json = created.data.into_json()?;
    let created_id = created_json["createScopedCollection"]["scopedCollection"]["id"]
        .as_str()
        .expect("missing created id")
        .to_string();

    let stored = ScopedCollection::get(&pool, &graphql_orm::uuid::Uuid::parse_str(&created_id)?)
        .await?
        .expect("stored row missing");
    assert_eq!(stored.owner_user_id, "actor-a");
    assert_eq!(stored.updated_by_user_id.as_deref(), Some("actor-a"));

    let second_created = admin_schema
        .execute(
            "mutation {
                createScopedCollection(input: { name: \"Bravo\" }) {
                    success
                    scopedCollection { id name }
                }
            }",
        )
        .await;
    assert!(second_created.errors.is_empty(), "{:?}", second_created.errors);

    let actor_a_list = actor_a_schema
        .execute(
            "{ scopedCollections(page: { limit: 1, offset: 0 }) { pageInfo { totalCount } edges { node { id name } } } }",
        )
        .await;
    assert!(actor_a_list.errors.is_empty(), "{:?}", actor_a_list.errors);
    let actor_a_list_json = actor_a_list.data.into_json()?;
    assert_eq!(
        actor_a_list_json["scopedCollections"]["edges"]
            .as_array()
            .expect("edges missing")
            .len(),
        1
    );
    assert_eq!(
        actor_a_list_json["scopedCollections"]["pageInfo"]["totalCount"].as_i64(),
        Some(1)
    );

    let actor_b_list = actor_b_schema
        .execute(
            "{ scopedCollections(page: { limit: 1, offset: 0 }) { pageInfo { totalCount } edges { node { id name } } } }",
        )
        .await;
    assert!(actor_b_list.errors.is_empty(), "{:?}", actor_b_list.errors);
    let actor_b_list_json = actor_b_list.data.into_json()?;
    assert_eq!(
        actor_b_list_json["scopedCollections"]["edges"]
            .as_array()
            .expect("edges missing")
            .len(),
        0
    );
    assert_eq!(
        actor_b_list_json["scopedCollections"]["pageInfo"]["totalCount"].as_i64(),
        Some(0)
    );

    let admin_list = admin_schema
        .execute(
            "{ scopedCollections(page: { limit: 1, offset: 0 }) { pageInfo { totalCount } edges { node { id name } } } }",
        )
        .await;
    assert!(admin_list.errors.is_empty(), "{:?}", admin_list.errors);
    let admin_list_json = admin_list.data.into_json()?;
    assert_eq!(
        admin_list_json["scopedCollections"]["edges"]
            .as_array()
            .expect("edges missing")
            .len(),
        1
    );
    assert_eq!(
        admin_list_json["scopedCollections"]["pageInfo"]["totalCount"].as_i64(),
        Some(2)
    );

    let actor_b_get = actor_b_schema
        .execute(format!(
            "{{ scopedCollection(id: \"{created_id}\") {{ id name }} }}"
        ))
        .await;
    assert!(actor_b_get.errors.is_empty(), "{:?}", actor_b_get.errors);
    let actor_b_get_json = actor_b_get.data.into_json()?;
    assert!(actor_b_get_json["scopedCollection"].is_null());

    let actor_a_update = actor_a_schema
        .execute(format!(
            "mutation {{
                updateScopedCollection(id: \"{created_id}\", input: {{ name: \"Renamed\" }}) {{
                    success
                    scopedCollection {{ id name }}
                }}
            }}"
        ))
        .await;
    assert!(
        actor_a_update.errors.is_empty(),
        "{:?}",
        actor_a_update.errors
    );
    let updated = ScopedCollection::get(&pool, &graphql_orm::uuid::Uuid::parse_str(&created_id)?)
        .await?
        .expect("updated row missing");
    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.updated_by_user_id.as_deref(), Some("actor-a"));

    let actor_b_update = actor_b_schema
        .execute(format!(
            "mutation {{
                updateScopedCollection(id: \"{created_id}\", input: {{ name: \"Blocked\" }}) {{
                    success
                    error
                }}
            }}"
        ))
        .await;
    let actor_b_update_json = actor_b_update.data.into_json()?;
    assert_eq!(
        actor_b_update_json["updateScopedCollection"]["success"]
            .as_bool()
            .unwrap_or(false),
        false
    );

    let repo_denied = ScopedCollection::update_by_id(
        &db,
        &graphql_orm::uuid::Uuid::parse_str(&created_id)?,
        UpdateScopedCollectionInput {
            name: Some("RepoBlocked".to_string()),
            owner_user_id: None,
            updated_by_user_id: Some(Some("repo-user".to_string())),
        },
    )
    .await;
    assert!(repo_denied.is_err());

    let admin_delete = admin_schema
        .execute(format!(
            "mutation {{
                deleteScopedCollection(id: \"{created_id}\") {{
                    success
                }}
            }}"
        ))
        .await;
    assert!(admin_delete.errors.is_empty(), "{:?}", admin_delete.errors);
    let admin_delete_json = admin_delete.data.into_json()?;
    assert_eq!(
        admin_delete_json["deleteScopedCollection"]["success"]
            .as_bool()
            .unwrap_or(false),
        true
    );
    assert!(
        ScopedCollection::get(&pool, &graphql_orm::uuid::Uuid::parse_str(&created_id)?)
            .await?
            .is_none()
    );

    let audit = transform.snapshot();
    assert!(audit.iter().any(|entry| entry == "create:actor-a"));
    assert!(audit.iter().any(|entry| entry == "update:actor-a"));

    Ok(())
}
