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
#[graphql_entity(
    table = "test_policies",
    plural = "Policies",
    default_sort = "name ASC"
)]
#[graphql(complex)]
struct Policy {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[graphql(skip)]
    #[relation(target = "StaffPolicy", from = "id", to = "policy_id", multiple)]
    pub staff_policies: Vec<StaffPolicy>,
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
#[graphql_entity(
    table = "test_staff",
    plural = "StaffMembers",
    default_sort = "name ASC"
)]
#[graphql(complex)]
struct Staff {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[graphql(skip)]
    #[relation(target = "StaffPolicy", from = "id", to = "staff_id", multiple)]
    pub staff_policies: Vec<StaffPolicy>,
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
#[graphql_entity(
    table = "test_staff_policies",
    plural = "StaffPolicies",
    default_sort = "staff_id ASC"
)]
#[graphql(complex)]
struct StaffPolicy {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub policy_id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub staff_id: String,

    #[graphql(skip)]
    #[relation(target = "Policy", from = "policy_id", to = "id")]
    pub policy: Option<Policy>,

    #[graphql(skip)]
    #[relation(target = "Staff", from = "staff_id", to = "id")]
    pub staff: Option<Staff>,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for Policy {
    fn batch_column() -> &'static str {
        "id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("id")
    }
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for Staff {
    fn batch_column() -> &'static str {
        "id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("id")
    }
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for StaffPolicy {
    fn batch_column() -> &'static str {
        "policy_id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("policy_id")
    }
}

schema_roots! {
    query_custom_ops: [],
    entities: [Policy, Staff, StaffPolicy],
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
    Ok(pool)
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test".to_string()
    });
    let pool = sqlx::PgPool::connect(&database_url).await?;
    for table in [
        "test_staff_policies",
        "test_staff",
        "test_policies",
        "__graphql_orm_migrations",
    ] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .execute(&pool)
            .await?;
    }
    Ok(pool)
}

async fn apply_schema(
    database: &graphql_orm::db::Database,
) -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{DatabaseBackend, Entity, Migration, build_migration_plan};

    let target_schema = graphql_orm::graphql::orm::SchemaModel::from_entities(&[
        <Policy as Entity>::metadata(),
        <Staff as Entity>::metadata(),
        <StaffPolicy as Entity>::metadata(),
    ]);
    let plan = build_migration_plan(
        if cfg!(feature = "postgres") {
            DatabaseBackend::Postgres
        } else {
            DatabaseBackend::Sqlite
        },
        &graphql_orm::graphql::orm::SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
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
            version: "2026042201_bidirectional_join_relations",
            description: "bidirectional_join_relations",
            statements,
        }])
        .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn insert_policy(pool: &sqlx::SqlitePool, id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO test_policies (id, name) VALUES (?, ?)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_policy(pool: &sqlx::PgPool, id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO test_policies (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn insert_staff(pool: &sqlx::SqlitePool, id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO test_staff (id, name) VALUES (?, ?)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_staff(pool: &sqlx::PgPool, id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO test_staff (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn insert_staff_policy(
    pool: &sqlx::SqlitePool,
    id: &str,
    policy_id: &str,
    staff_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO test_staff_policies (id, policy_id, staff_id) VALUES (?, ?, ?)")
        .bind(id)
        .bind(policy_id)
        .bind(staff_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_staff_policy(
    pool: &sqlx::PgPool,
    id: &str,
    policy_id: &str,
    staff_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO test_staff_policies (id, policy_id, staff_id) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(policy_id)
        .bind(staff_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn reverse_join_relations_support_nested_queries_and_counts()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    apply_schema(&database).await?;

    insert_policy(&pool, "policy-1", "HQ Access").await?;
    insert_staff(&pool, "staff-1", "Ada Lovelace").await?;
    insert_staff(&pool, "staff-2", "Grace Hopper").await?;
    insert_staff_policy(&pool, "link-1", "policy-1", "staff-1").await?;
    insert_staff_policy(&pool, "link-2", "policy-1", "staff-2").await?;

    let schema = schema_builder(database).data("viewer".to_string()).finish();
    let response = schema
        .execute(
            r#"
            query {
              policy(id: "policy-1") {
                id
                staffPolicies(
                  page: { limit: 10, offset: 0 }
                  orderBy: { staffId: ASC }
                ) {
                  pageInfo {
                    totalCount
                  }
                  edges {
                    node {
                      staffId
                      staff {
                        id
                        name
                      }
                    }
                  }
                }
              }
              staff(id: "staff-1") {
                id
                staffPolicies(
                  page: { limit: 10, offset: 0 }
                  where: { policyId: { eq: "policy-1" } }
                ) {
                  pageInfo {
                    totalCount
                  }
                  edges {
                    node {
                      policy {
                        id
                        name
                      }
                    }
                  }
                }
              }
            }
            "#,
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    let json = response.data.into_json()?;
    assert_eq!(
        json["policy"]["staffPolicies"]["pageInfo"]["totalCount"].as_i64(),
        Some(2)
    );
    assert_eq!(
        json["policy"]["staffPolicies"]["edges"][0]["node"]["staff"]["name"].as_str(),
        Some("Ada Lovelace")
    );
    assert_eq!(
        json["policy"]["staffPolicies"]["edges"][1]["node"]["staff"]["name"].as_str(),
        Some("Grace Hopper")
    );
    assert_eq!(
        json["staff"]["staffPolicies"]["pageInfo"]["totalCount"].as_i64(),
        Some(1)
    );
    assert_eq!(
        json["staff"]["staffPolicies"]["edges"][0]["node"]["policy"]["name"].as_str(),
        Some("HQ Access")
    );

    Ok(())
}
