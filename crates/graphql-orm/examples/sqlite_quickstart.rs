//! A small, intentionally public SQLite GraphQL service.
//!
//! Run it from the repository root with:
//! `cargo run -p graphql-orm --example sqlite_quickstart`
//!
//! The `auth: "none"` setting below is only for a local learning service. A
//! deployed service must install an application-owned authentication and
//! authorization policy before exposing generated operations.

use async_graphql::Request;
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "tasks",
    plural = "Tasks",
    default_sort = "title ASC",
    auth = "none"
)]
struct Task {
    #[primary_key]
    id: String,

    #[filterable(type = "string")]
    #[sortable]
    title: String,

    #[filterable(type = "boolean")]
    completed: bool,
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    auth: "none",
    query_custom_ops: [],
    entities: [Task],
}

async fn build_schema(database_url: &str) -> Result<AppSchema, Box<dyn std::error::Error>> {
    let database = Database::<SqliteBackend>::connect_sqlite(database_url)
        .await?
        .with_schema_policy(SchemaPolicy::Managed);

    // Schema application is explicit: constructing a Database or GraphQL
    // schema never changes the database by itself.
    let plan = database
        .schema()
        .plan_migration_to_entities("quickstart-001", "create tasks", &[Task::metadata()])
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    Task::insert_many(
        &database,
        [
            CreateTaskInput {
                title: "Learn graphql-orm".to_owned(),
                completed: false,
            },
            CreateTaskInput {
                title: "Run the quickstart".to_owned(),
                completed: true,
            },
        ],
    )
    .await?;

    Ok(schema_builder(database).finish())
}

async fn graphql(
    State(schema): State<AppSchema>,
    Json(request): Json<Request>,
) -> Json<async_graphql::Response> {
    Json(schema.execute(request).await)
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema = build_schema("sqlite://quickstart.db?mode=rwc").await?;
    let app = Router::new()
        .route("/graphql", post(graphql))
        .route("/health", get(health))
        .with_state(schema);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;

    println!("GraphQL demo listening at http://127.0.0.1:3000/graphql");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn smoke_test_runs_the_generated_tasks_query() -> Result<(), Box<dyn std::error::Error>> {
        let schema = build_schema("sqlite::memory:").await?;
        let response = schema
            .execute("{ tasks { edges { node { id title completed } } pageInfo { totalCount } } }")
            .await;

        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            response.data.into_json()?["tasks"]["pageInfo"]["totalCount"],
            2
        );
        Ok(())
    }
}
