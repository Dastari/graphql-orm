//! Full-stack generated schema fixture using the production runtime.
use async_graphql::SimpleObject;
use graphql_orm::prelude::*;

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
#[graphql_entity(
    table = "users",
    plural = "Users",
    default_sort = "name ASC",
    keyset = "name asc, id asc",
    aggregate = true,
    aggregate_source = true,
    federation_key(fields = ["id"]),
    node_id
)]
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
    #[graphql_orm(searchable(weight = "A"))]
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

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "editions",
    plural = "Editions",
    keyset = "title asc, number asc"
)]
pub struct Edition {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub title: String,
    #[primary_key]
    pub number: i64,
}

schema_roots! {
    query_custom_ops: [],
    client_metadata: true,
    entities: [User, Post, Edition],
}

/// Build SDL without a database or executing any resolver.
pub fn export_sdl() -> String {
    async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .finish()
    .sdl()
}

#[cfg(feature = "sqlite")]
type DbRow = sqlx::sqlite::SqliteRow;
#[cfg(feature = "postgres")]
type DbRow = sqlx::postgres::PgRow;

impl BatchLoadEntity for User {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(row: &DbRow) -> Result<String, sqlx::Error> {
        use sqlx::Row;
        row.try_get("id")
    }
}

impl BatchLoadEntity for Post {
    fn batch_column() -> &'static str {
        "author_id"
    }
    fn batch_key_from_row(row: &DbRow) -> Result<String, sqlx::Error> {
        use sqlx::Row;
        row.try_get("author_id")
    }
}

/// Repository-only fixture: its policy-aware list body is rewritten before export stubbing.
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "export_records", plural = "ExportRecords")]
pub struct ExportRecord {
    #[primary_key]
    pub id: String,
}

#[cfg(all(test, feature = "schema-export-only", feature = "sqlite"))]
mod export_tests {
    use super::*;

    #[tokio::test]
    #[should_panic(expected = "schema-export-only generated body must not be called")]
    async fn repository_rewritten_list_body_also_panics() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let db = graphql_orm::db::Database::<SqliteBackend>::new(pool);
        let _ = ExportRecord::find_all(&db).await;
    }
}

#[cfg(all(test, feature = "sqlite"))]
#[test]
fn query_argument_spelling_preserves_catalogue_fingerprints() {
    // The artifact was captured with the default naming rules.
    if [
        option_env!("GRAPHQL_ORM_RESOLVER_CASE"),
        option_env!("GRAPHQL_ORM_ARGUMENT_CASE"),
        option_env!("GRAPHQL_ORM_FIELD_CASE"),
    ]
    .into_iter()
    .flatten()
    .any(|rule| !matches!(rule, "" | "camelCase"))
    {
        return;
    }
    let catalog = graphql_orm_operation_catalog();
    let actual = [
        catalog.fingerprint(),
        catalog.authorization_fingerprint(),
        catalog.router_export_fingerprint(),
    ];
    let expected: Vec<_> = include_str!("../catalogue-fingerprints-sqlite.txt")
        .lines()
        .collect();
    assert_eq!(actual.as_slice(), expected.as_slice());
}
