#![cfg(feature = "postgres")]

use async_graphql::{Schema, SimpleObject};
use graphql_orm::prelude::*;
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;
use std::sync::OnceLock;

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
#[graphql_entity(table = "users", plural = "Users", default_sort = "name ASC")]
#[graphql(complex)]
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
#[graphql_entity(table = "posts", plural = "Posts", default_sort = "title ASC")]
#[graphql(complex)]
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

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "pg_type_probes",
    plural = "PgTypeProbes",
    default_sort = "created_at ASC"
)]
pub struct PgTypeProbe {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub details: serde_json::Value,

    #[date_field]
    pub observed_at: String,

    pub created_at: i64,
}

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "indexed_parents",
    plural = "IndexedParents",
    default_sort = "tenant_id ASC"
)]
pub struct IndexedParent {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    pub tenant_id: String,

    #[graphql(skip)]
    #[relation(
        target = "IndexedChild",
        from = "id",
        to = "parent_id",
        multiple,
        emit_fk = false
    )]
    pub children: Vec<IndexedChild>,
}

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(
    table = "indexed_children",
    plural = "IndexedChildren",
    default_sort = "label ASC"
)]
pub struct IndexedChild {
    #[primary_key]
    pub id: String,

    pub parent_id: String,

    pub label: String,
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

fn test_mutex() -> &'static tokio::sync::Mutex<()> {
    static TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    TEST_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn setup_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test".to_string()
    });
    let pool = sqlx::PgPool::connect(&database_url).await?;

    sqlx::query("DROP TABLE IF EXISTS posts")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS users")
        .execute(&pool)
        .await?;

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
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            FOREIGN KEY (author_id) REFERENCES users(id)
        )",
    )
    .execute(&pool)
    .await?;

    Ok(pool)
}

fn has_index_on(table: &graphql_orm::graphql::orm::TableModel, columns: &[&str]) -> bool {
    table.indexes.iter().any(|index| {
        index.columns.len() == columns.len()
            && index
                .columns
                .iter()
                .zip(columns.iter())
                .all(|(left, right)| left == right)
    })
}

#[test]
fn postgres_schema_model_covers_types_and_generated_indexes() {
    use graphql_orm::graphql::orm::{DatabaseBackend, Entity, SchemaModel, build_migration_plan};

    let probe_metadata = <PgTypeProbe as Entity>::metadata();
    let probe_fields = probe_metadata
        .fields
        .iter()
        .map(|field| (field.name, field.sql_type))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(probe_fields.get("id"), Some(&"UUID"));
    assert_eq!(probe_fields.get("details"), Some(&"JSONB"));
    assert_eq!(probe_fields.get("observed_at"), Some(&"TIMESTAMPTZ"));
    assert_eq!(probe_fields.get("created_at"), Some(&"BIGINT"));

    let schema_model = SchemaModel::from_entities(&[
        probe_metadata,
        <IndexedParent as Entity>::metadata(),
        <IndexedChild as Entity>::metadata(),
    ]);
    let indexed_parents = schema_model
        .tables
        .iter()
        .find(|table| table.table_name == "indexed_parents")
        .expect("indexed parent table should exist");
    assert!(
        has_index_on(indexed_parents, &["tenant_id"]),
        "filterable columns should get generated indexes"
    );
    let indexed_children = schema_model
        .tables
        .iter()
        .find(|table| table.table_name == "indexed_children")
        .expect("indexed child table should exist");
    assert!(
        has_index_on(indexed_children, &["parent_id"]),
        "has-many relation lookup columns should get generated indexes"
    );

    let plan = build_migration_plan(
        DatabaseBackend::Postgres,
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &schema_model,
    );
    assert!(plan.statements.iter().any(|statement| {
        statement.contains("CREATE TABLE pg_type_probes")
            && statement.contains("id UUID")
            && statement.contains("details JSONB")
            && statement.contains("observed_at TIMESTAMPTZ")
            && statement.contains("created_at BIGINT")
    }));
    assert!(plan.statements.iter().any(|statement| {
        statement == "CREATE INDEX idx_indexed_parents_tenant_id ON indexed_parents (tenant_id)"
    }));
    assert!(plan.statements.iter().any(|statement| {
        statement == "CREATE INDEX idx_indexed_children_parent_id ON indexed_children (parent_id)"
    }));
}

#[tokio::test]
async fn current_macros_work_against_graphql_orm_runtime() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;

    let database = graphql_orm::db::Database::new(pool.clone());
    let schema: TestSchema = schema_builder(database)
        .data("test-user".to_string())
        .finish();

    let create_user = schema
        .execute(
            "mutation {
                createUser(input: { name: \"Alice\", active: true }) {
                    success
                    user { id name }
                }
            }",
        )
        .await;
    assert!(create_user.errors.is_empty(), "{:?}", create_user.errors);
    let user_json = create_user.data.into_json()?;
    let user_id = user_json["createUser"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let create_post = schema
        .execute(format!(
            "mutation {{
                createPost(input: {{ authorId: \"{user_id}\", title: \"Hello\", published: true }}) {{
                    success
                    post {{ id title }}
                }}
            }}"
        ))
        .await;
    assert!(create_post.errors.is_empty(), "{:?}", create_post.errors);

    let nested = schema
        .execute(
            "query {
                users {
                    edges {
                        node {
                            name
                            posts {
                                edges { node { title } }
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
        nested_json["users"]["edges"][0]["node"]["name"].as_str(),
        Some("Alice")
    );
    assert_eq!(
        nested_json["users"]["edges"][0]["node"]["posts"]["edges"][0]["node"]["title"].as_str(),
        Some("Hello")
    );

    let metadata = <User as graphql_orm::graphql::orm::Entity>::metadata();
    assert_eq!(metadata.entity_name, "User");
    assert_eq!(metadata.table_name, "users");
    assert_eq!(metadata.primary_key, "id");
    assert_eq!(metadata.fields.len(), 5);
    assert_eq!(metadata.relations.len(), 1);
    assert_eq!(metadata.relations[0].field_name, "posts");
    assert_eq!(metadata.relations[0].source_column, "id");
    assert_eq!(metadata.relations[0].target_column, "author_id");
    let schema_model = graphql_orm::graphql::orm::SchemaModel::from_entities(&[metadata]);
    assert_eq!(schema_model.tables.len(), 1);
    assert_eq!(schema_model.tables[0].table_name, "users");
    assert_eq!(schema_model.tables[0].foreign_keys.len(), 0);

    let introspected = graphql_orm::graphql::orm::introspect_schema(&pool).await?;
    assert!(introspected.tables.len() >= 2);
    let users_table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "users")
        .expect("users table should be introspected");
    assert_eq!(users_table.primary_key, "id");
    assert!(
        users_table
            .columns
            .iter()
            .any(|column| column.name == "name")
    );
    let posts_table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "posts")
        .expect("posts table should be introspected");
    assert!(
        posts_table
            .columns
            .iter()
            .any(|column| column.name == "author_id")
    );

    assert!(matches!(
        graphql_orm::graphql::orm::current_backend(),
        graphql_orm::graphql::orm::DatabaseBackend::Postgres
    ));

    Ok(())
}

#[tokio::test]
async fn relation_resolvers_batch_for_pages_of_parents() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool);
    let schema: TestSchema = schema_builder(database)
        .data("test-user".to_string())
        .finish();

    let mut user_ids = Vec::new();
    for name in ["Alice", "Bob", "Cara", "Dana"] {
        let response = schema
            .execute(format!(
                "mutation {{
                    createUser(input: {{ name: \"{name}\", active: true }}) {{
                        user {{ id }}
                    }}
                }}"
            ))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json()?;
        user_ids.push(
            data["createUser"]["user"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }

    for (author_id, title, published) in [
        (user_ids[0].clone(), "A1", true),
        (user_ids[0].clone(), "A3", true),
        (user_ids[0].clone(), "A2", true),
        (user_ids[1].clone(), "B1", false),
        (user_ids[1].clone(), "B2", true),
        (user_ids[2].clone(), "C1", true),
        (user_ids[3].clone(), "D1", true),
    ] {
        let response = schema
            .execute(format!(
                "mutation {{
                    createPost(input: {{ authorId: \"{author_id}\", title: \"{title}\", published: {} }}) {{
                        success
                    }}
                }}",
                if published { "true" } else { "false" }
            ))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
    }

    graphql_orm::graphql::orm::reset_query_count();

    let no_args = schema
        .execute(
            "query {
                users(orderBy: [{ name: ASC }]) {
                    edges {
                        node {
                            name
                            posts {
                                edges { node { title } }
                            }
                        }
                    }
                }
            }",
        )
        .await;
    assert!(no_args.errors.is_empty(), "{:?}", no_args.errors);
    let no_args_json = no_args.data.into_json()?;
    let edges = no_args_json["users"]["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 4);
    assert!(
        graphql_orm::graphql::orm::query_count() < edges.len() + 2,
        "expected no-args has-many relation loading to stay below N+1; got {} for {} parent rows",
        graphql_orm::graphql::orm::query_count(),
        edges.len()
    );

    graphql_orm::graphql::orm::reset_query_count();

    let arg_query = schema
        .execute(
            "query {
                users(orderBy: [{ name: ASC }]) {
                    edges {
                        node {
                            name
                            posts(
                                where: { published: { eq: true } }
                                orderBy: { title: DESC }
                                page: { limit: 1, offset: 0 }
                            ) {
                                edges { node { title } }
                                pageInfo { totalCount hasNextPage }
                            }
                        }
                    }
                }
            }",
        )
        .await;
    assert!(arg_query.errors.is_empty(), "{:?}", arg_query.errors);
    let arg_json = arg_query.data.into_json()?;
    let arg_edges = arg_json["users"]["edges"].as_array().unwrap();
    assert_eq!(arg_edges.len(), 4);
    assert_eq!(
        arg_edges[0]["node"]["posts"]["edges"][0]["node"]["title"].as_str(),
        Some("A3")
    );
    assert_eq!(
        arg_edges[0]["node"]["posts"]["pageInfo"]["totalCount"].as_i64(),
        Some(3)
    );
    assert!(
        graphql_orm::graphql::orm::query_count() < arg_edges.len() + 2,
        "expected arg-aware has-many relation loading to stay below N+1; got {} for {} parent rows",
        graphql_orm::graphql::orm::query_count(),
        arg_edges.len()
    );

    graphql_orm::graphql::orm::reset_query_count();

    let belongs_to = schema
        .execute(
            "query {
                posts(orderBy: [{ title: ASC }]) {
                    edges {
                        node {
                            title
                            author { name }
                        }
                    }
                }
            }",
        )
        .await;
    assert!(belongs_to.errors.is_empty(), "{:?}", belongs_to.errors);
    let belongs_to_json = belongs_to.data.into_json()?;
    let post_edges = belongs_to_json["posts"]["edges"].as_array().unwrap();
    assert_eq!(post_edges.len(), 7);
    assert!(
        graphql_orm::graphql::orm::query_count() < post_edges.len() + 2,
        "expected belongs-to relation loading to stay below N+1; got {} for {} parent rows",
        graphql_orm::graphql::orm::query_count(),
        post_edges.len()
    );

    Ok(())
}

#[tokio::test]
async fn page_limit_cap_applies_to_top_level_and_nested_relation_lists()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::builder(pool)
        .max_page_limit(Some(2))
        .build();
    let schema: TestSchema = schema_builder(database)
        .data("test-user".to_string())
        .finish();

    let mut user_ids = Vec::new();
    for name in ["Alice", "Bob", "Cara"] {
        let response = schema
            .execute(format!(
                "mutation {{
                    createUser(input: {{ name: \"{name}\", active: true }}) {{
                        user {{ id }}
                    }}
                }}"
            ))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json()?;
        user_ids.push(
            data["createUser"]["user"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }

    for title in ["A1", "A2", "A3"] {
        let response = schema
            .execute(format!(
                "mutation {{
                    createPost(input: {{ authorId: \"{}\", title: \"{title}\", published: true }}) {{
                        success
                    }}
                }}",
                user_ids[0]
            ))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
    }
    let response = schema
        .execute(format!(
            "mutation {{
                createPost(input: {{ authorId: \"{}\", title: \"B1\", published: true }}) {{
                    success
                }}
            }}",
            user_ids[1]
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    let response = schema
        .execute(
            "query {
                users(orderBy: [{ name: ASC }], page: { limit: 10 }) {
                    edges {
                        node {
                            name
                            posts(orderBy: { title: ASC }, page: { limit: 10 }) {
                                edges { node { title } }
                                pageInfo { totalCount hasNextPage }
                            }
                        }
                    }
                    pageInfo { totalCount hasNextPage }
                }
            }",
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json()?;
    let users = data["users"]["edges"].as_array().unwrap();
    assert_eq!(users.len(), 2, "top-level list should be capped");
    assert_eq!(data["users"]["pageInfo"]["totalCount"].as_i64(), Some(3));
    assert_eq!(
        data["users"]["pageInfo"]["hasNextPage"].as_bool(),
        Some(true)
    );
    assert_eq!(users[0]["node"]["name"].as_str(), Some("Alice"));

    let alice_posts = users[0]["node"]["posts"]["edges"].as_array().unwrap();
    assert_eq!(
        alice_posts.len(),
        2,
        "nested relation list should be capped"
    );
    assert_eq!(
        users[0]["node"]["posts"]["pageInfo"]["totalCount"].as_i64(),
        Some(3)
    );
    assert_eq!(
        users[0]["node"]["posts"]["pageInfo"]["hasNextPage"].as_bool(),
        Some(true)
    );

    Ok(())
}

#[tokio::test]
async fn postgres_introspection_uses_active_schema_search_path()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test".to_string()
    });
    let admin_pool = sqlx::PgPool::connect(&database_url).await?;
    let schema_name = format!(
        "gom_active_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema_name} CASCADE"))
        .execute(&admin_pool)
        .await?;
    sqlx::query(&format!("CREATE SCHEMA {schema_name}"))
        .execute(&admin_pool)
        .await?;

    let schema_for_hook = schema_name.clone();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |connection, _metadata| {
            let schema_name = schema_for_hook.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {schema_name}, public"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&database_url)
        .await?;

    sqlx::query(
        "CREATE TABLE parents (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE children (
            id TEXT PRIMARY KEY,
            parent_id TEXT NOT NULL,
            payload JSONB NOT NULL,
            observed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            FOREIGN KEY (parent_id) REFERENCES parents(id)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE INDEX idx_children_parent_id ON children (parent_id)")
        .execute(&pool)
        .await?;

    let active_schema = sqlx::query("SELECT current_schema() AS schema_name")
        .fetch_one(&pool)
        .await?
        .try_get::<String, _>("schema_name")?;
    assert_eq!(active_schema, schema_name);

    let introspected = graphql_orm::graphql::orm::introspect_schema(&pool).await?;
    assert!(
        introspected
            .tables
            .iter()
            .any(|table| table.table_name == "parents"),
        "active non-public schema should be introspected"
    );
    assert!(
        !introspected
            .tables
            .iter()
            .any(|table| table.table_name == "users"),
        "tables from public should not leak into active-schema introspection"
    );
    let children = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "children")
        .expect("children table should be introspected from active schema");
    assert!(
        children.indexes.iter().any(|index| {
            index.name == "idx_children_parent_id" && index.columns == ["parent_id"]
        })
    );
    assert!(children.foreign_keys.iter().any(|foreign_key| {
        foreign_key.source_column == "parent_id"
            && foreign_key.target_table == "parents"
            && foreign_key.target_column == "id"
    }));
    assert!(children.columns.iter().any(|column| {
        column.name == "payload" && column.sql_type.eq_ignore_ascii_case("jsonb")
    }));
    assert!(children.columns.iter().any(|column| {
        column.name == "observed_at" && column.sql_type == "timestamp with time zone"
    }));

    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema_name} CASCADE"))
        .execute(&admin_pool)
        .await?;

    Ok(())
}
