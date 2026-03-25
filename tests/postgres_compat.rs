#![cfg(feature = "postgres")]

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
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&database_url).await?;

    sqlx::query("DROP TABLE IF EXISTS posts").execute(&pool).await?;
    sqlx::query("DROP TABLE IF EXISTS users").execute(&pool).await?;

    sqlx::query(
        "CREATE TABLE users (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            active BOOLEAN NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE posts (
            id TEXT PRIMARY KEY,
            author_id TEXT NOT NULL,
            title TEXT NOT NULL,
            published BOOLEAN NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
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

    Ok(())
}
