#![cfg(feature = "sqlite")]

use async_graphql::dataloader::DataLoader;
use async_graphql::{Schema, SimpleObject};
use graphql_orm::prelude::*;
use sqlx::Row;

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql(complex)]
#[graphql_entity(table = "users", plural = "Users", default_sort = "name ASC")]
pub struct User {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "boolean")]
    pub active: bool,

    #[filterable(type = "number")]
    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,

    #[graphql(skip)]
    #[relation(target = "Post", from = "id", to = "author_id", multiple)]
    pub posts: Vec<Post>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql(complex)]
#[graphql_entity(table = "posts", plural = "Posts", default_sort = "title ASC")]
pub struct Post {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    pub author_id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub title: String,

    #[filterable(type = "boolean")]
    pub published: bool,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,

    #[graphql(skip)]
    #[relation(target = "User", from = "author_id", to = "id")]
    pub author: Option<User>,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for User {
    fn batch_column() -> &'static str {
        "id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("id")
    }
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for Post {
    fn batch_column() -> &'static str {
        "author_id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("author_id")
    }
}

schema_roots! {
    query_custom_ops: [],
    entities: [User, Post],
}

type TestSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

#[tokio::test]
async fn current_macros_work_against_graphql_orm_runtime() -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    sqlx::query("PRAGMA foreign_keys = ON").execute(&pool).await?;

    sqlx::query(
        "CREATE TABLE users (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            active INTEGER NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE posts (
            id TEXT PRIMARY KEY,
            author_id TEXT NOT NULL,
            title TEXT NOT NULL,
            published INTEGER NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
            FOREIGN KEY (author_id) REFERENCES users(id)
        )",
    )
    .execute(&pool)
    .await?;

    let database = graphql_orm::db::Database::new(pool.clone());
    let schema: TestSchema =
        Schema::build(QueryRoot::default(), MutationRoot::default(), SubscriptionRoot::default())
        .data(database.clone())
        .data("test-user".to_string())
        .data(DataLoader::new(
            graphql_orm::graphql::loaders::RelationLoader::<User>::new(database.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            graphql_orm::graphql::loaders::RelationLoader::<Post>::new(database),
            tokio::spawn,
        ))
        .finish();

    let create_user = schema
        .execute(
            "mutation {
                CreateUser(Input: { Name: \"Alice\", Active: true }) {
                    Success
                    User { id name }
                }
            }",
        )
        .await;
    assert!(create_user.errors.is_empty(), "{:?}", create_user.errors);
    let user_json = create_user.data.into_json()?;
    let user_id = user_json["CreateUser"]["User"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let create_post = schema
        .execute(format!(
            "mutation {{
                CreatePost(Input: {{ AuthorId: \"{user_id}\", Title: \"Hello\", Published: true }}) {{
                    Success
                    Post {{ id title }}
                }}
            }}"
        ))
        .await;
    assert!(create_post.errors.is_empty(), "{:?}", create_post.errors);

    let nested = schema
        .execute(
            "query {
                Users {
                    Edges {
                        Node {
                            name
                            Posts {
                                Edges { Node { title } }
                            }
                        }
                    }
                }
            }",
        )
        .await;
    assert!(nested.errors.is_empty(), "{:?}", nested.errors);
    let nested_json = nested.data.into_json()?;
    assert_eq!(
        nested_json["Users"]["Edges"][0]["Node"]["name"].as_str(),
        Some("Alice")
    );
    assert_eq!(
        nested_json["Users"]["Edges"][0]["Node"]["Posts"]["Edges"][0]["Node"]["title"].as_str(),
        Some("Hello")
    );

    let metadata = <User as graphql_orm::graphql::orm::Entity>::metadata();
    assert_eq!(metadata.entity_name, "User");
    assert_eq!(metadata.table_name, "users");
    assert_eq!(metadata.primary_key, "id");
    assert_eq!(metadata.fields.len(), 5);
    assert_eq!(metadata.relations.len(), 1);
    assert_eq!(metadata.relations[0].field_name, "Posts");
    assert_eq!(metadata.relations[0].source_column, "id");
    assert_eq!(metadata.relations[0].target_column, "author_id");
    let schema_model =
        graphql_orm::graphql::orm::SchemaModel::from_entities(&[metadata]);
    assert_eq!(schema_model.tables.len(), 1);
    assert_eq!(schema_model.tables[0].table_name, "users");
    assert_eq!(schema_model.tables[0].foreign_keys.len(), 1);
    assert_eq!(schema_model.tables[0].foreign_keys[0].target_column, "author_id");
    let introspected = graphql_orm::graphql::orm::introspect_schema(&pool).await?;
    assert_eq!(introspected.tables.len(), 2);
    let users_table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "users")
        .expect("users table should be introspected");
    assert_eq!(users_table.primary_key, "id");
    assert!(users_table.columns.iter().any(|column| column.name == "name"));
    let posts_table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "posts")
        .expect("posts table should be introspected");
    assert!(posts_table
        .columns
        .iter()
        .any(|column| column.name == "author_id"));
    assert!(posts_table.foreign_keys.iter().any(|foreign_key| {
        foreign_key.source_column == "author_id"
            && foreign_key.target_table == "users"
            && foreign_key.target_column == "id"
    }));
    assert!(matches!(
        graphql_orm::graphql::orm::current_backend(),
        graphql_orm::graphql::orm::DatabaseBackend::Sqlite
    ));

    Ok(())
}
