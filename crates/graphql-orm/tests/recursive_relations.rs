use async_graphql::SimpleObject;
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
#[graphql_entity(table = "places", plural = "Places", default_sort = "name ASC")]
#[graphql(complex)]
struct Place {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "string")]
    pub parent_id: Option<String>,

    #[graphql(skip)]
    #[relation(target = "Place", from = "parent_id", to = "id")]
    pub parent_place: Option<Box<Place>>,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for Place {
    fn batch_column() -> &'static str {
        "id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("id")
    }
}

schema_roots! {
    query_custom_ops: [],
    entities: [Place],
}

#[cfg(feature = "sqlite")]
type TestPool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type TestPool = sqlx::PgPool;

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    Ok(sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?)
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&database_url).await?;
    sqlx::query("DROP TABLE IF EXISTS places")
        .execute(&pool)
        .await?;
    Ok(pool)
}

async fn apply_schema(
    database: &graphql_orm::db::Database,
) -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{DatabaseBackend, Entity, Migration, build_migration_plan};

    let target_schema =
        graphql_orm::graphql::orm::SchemaModel::from_entities(&[<Place as Entity>::metadata()]);
    let plan = build_migration_plan(
        if cfg!(feature = "postgres") {
            DatabaseBackend::Postgres
        } else {
            DatabaseBackend::Sqlite
        },
        &graphql_orm::graphql::orm::SchemaModel { tables: Vec::new() },
        &target_schema,
    );
    let statements: &'static [&'static str] = Box::leak(
        plan.statements
            .iter()
            .map(|statement| Box::leak(statement.clone().into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    database
        .apply_migrations(&[Migration {
            version: "2026040103_recursive_places",
            description: "recursive_places",
            statements,
        }])
        .await?;
    Ok(())
}

#[tokio::test]
async fn self_referential_relations_work_with_boxed_parent_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    apply_schema(&database).await?;

    let root = Place::insert(
        &pool,
        CreatePlaceInput {
            name: "Root".to_string(),
            parent_id: None,
        },
    )
    .await?;
    let child = Place::insert(
        &pool,
        CreatePlaceInput {
            name: "Child".to_string(),
            parent_id: Some(root.id.clone()),
        },
    )
    .await?;

    let schema = schema_builder(database).data("viewer".to_string()).finish();
    let response = schema
        .execute(format!(
            "query {{
                place(id: \"{}\") {{
                    id
                    name
                    parentPlace {{
                        id
                        name
                    }}
                }}
            }}",
            child.id
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json()?;
    assert_eq!(json["place"]["name"].as_str(), Some("Child"));
    assert_eq!(
        json["place"]["parentPlace"]["id"].as_str(),
        Some(root.id.as_str())
    );
    assert_eq!(json["place"]["parentPlace"]["name"].as_str(), Some("Root"));

    Ok(())
}
