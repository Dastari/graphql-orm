#![cfg(feature = "sqlite")]

use graphql_orm::async_graphql::{Schema, SimpleObject};
use graphql_orm::prelude::*;
use graphql_orm::sqlx::Row;

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
#[graphql(rename_fields = "PascalCase")]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "conditional_work_items",
    plural = "ConditionalWorkItems",
    schema_policy = "external_read_only",
    default_sort = "id ASC"
)]
struct ConditionalWorkItem {
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    id: i32,

    #[filterable(type = "number")]
    #[sortable]
    kind: i32,

    #[filterable(type = "number")]
    #[sortable]
    ref_no: i32,

    #[filterable(type = "string")]
    kind_label: String,

    #[filterable(type = "boolean")]
    kind_enabled: Option<bool>,

    #[filterable(type = "number")]
    kind_score: f64,

    /// Related job when this record carries the job discriminator.
    #[graphql(skip)]
    #[relation(
        target = "ConditionalJob",
        from = "ref_no",
        to = "id",
        source_condition(field = "kind", equals = 0),
        emit_fk = false
    )]
    job: Option<ConditionalJob>,

    /// Related request when this record carries the request discriminator.
    #[graphql(skip)]
    #[relation(
        target = "ConditionalRequest",
        from = "ref_no",
        to = "id",
        source_condition(field = "kind", equals = 2),
        emit_fk = false
    )]
    request: Option<ConditionalRequest>,

    /// Related job when a string discriminator matches.
    #[graphql(skip)]
    #[relation(
        target = "ConditionalJob",
        from = "ref_no",
        to = "id",
        source_condition(field = "kind_label", equals = "job"),
        emit_fk = false
    )]
    labeled_job: Option<ConditionalJob>,

    /// Related job when an optional boolean discriminator matches.
    #[graphql(skip)]
    #[relation(
        target = "ConditionalJob",
        from = "ref_no",
        to = "id",
        source_condition(field = "kind_enabled", equals = true),
        emit_fk = false
    )]
    enabled_job: Option<ConditionalJob>,

    /// Related job when a floating-point discriminator matches.
    #[graphql(skip)]
    #[relation(
        target = "ConditionalJob",
        from = "ref_no",
        to = "id",
        source_condition(field = "kind_score", equals = 1.5),
        emit_fk = false
    )]
    scored_job: Option<ConditionalJob>,
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
#[graphql(rename_fields = "PascalCase")]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "conditional_jobs",
    plural = "ConditionalJobs",
    schema_policy = "external_read_only",
    default_sort = "id ASC"
)]
struct ConditionalJob {
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    id: i32,

    #[filterable(type = "string")]
    title: String,

    /// Work records whose fixed target discriminator identifies a job.
    #[graphql(skip)]
    #[relation(
        target = "ConditionalWorkItem",
        from = "id",
        to = "ref_no",
        target_condition(column = "kind", equals = 0),
        multiple,
        emit_fk = false
    )]
    work: Vec<ConditionalWorkItem>,
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
#[graphql(rename_fields = "PascalCase")]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "conditional_requests",
    plural = "ConditionalRequests",
    schema_policy = "external_read_only",
    default_sort = "id ASC"
)]
struct ConditionalRequest {
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    id: i32,

    #[filterable(type = "string")]
    title: String,

    /// Work records whose fixed target discriminator identifies a request.
    #[graphql(skip)]
    #[relation(
        target = "ConditionalWorkItem",
        from = "id",
        to = "ref_no",
        target_condition(column = "kind", equals = 2),
        multiple,
        emit_fk = false
    )]
    work: Vec<ConditionalWorkItem>,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::SqliteBackend>
    for ConditionalWorkItem
{
    fn batch_column() -> &'static str {
        "ref_no"
    }

    fn batch_key_from_row(
        row: &graphql_orm::sqlx::sqlite::SqliteRow,
    ) -> Result<String, sqlx::Error> {
        row.try_get::<i32, _>("ref_no")
            .map(|value| value.to_string())
    }
}

macro_rules! impl_batch_by_id {
    ($entity:ty) => {
        impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::SqliteBackend>
            for $entity
        {
            fn batch_column() -> &'static str {
                "id"
            }

            fn batch_key_from_row(
                row: &graphql_orm::sqlx::sqlite::SqliteRow,
            ) -> Result<String, sqlx::Error> {
                row.try_get::<i32, _>("id").map(|value| value.to_string())
            }
        }
    };
}

impl_batch_by_id!(ConditionalJob);
impl_batch_by_id!(ConditionalRequest);

schema_roots! {
    backend: "sqlite",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    entities: [ConditionalWorkItem, ConditionalJob, ConditionalRequest],
}

type TestSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

async fn setup_schema() -> Result<TestSchema, Box<dyn std::error::Error>> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::query(
        "CREATE TABLE conditional_work_items (
            id INTEGER PRIMARY KEY,
            kind INTEGER NOT NULL,
            ref_no INTEGER NOT NULL,
            kind_label TEXT NOT NULL,
            kind_enabled INTEGER,
            kind_score REAL NOT NULL
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query("CREATE TABLE conditional_jobs (id INTEGER PRIMARY KEY, title TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE TABLE conditional_requests (id INTEGER PRIMARY KEY, title TEXT NOT NULL)")
        .execute(&pool)
        .await?;

    sqlx::query("INSERT INTO conditional_jobs (id, title) VALUES (10, 'Job ten')")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO conditional_requests (id, title) VALUES (10, 'Request ten')")
        .execute(&pool)
        .await?;
    for (id, kind, kind_label, kind_enabled, kind_score) in [
        (1, 0, "job", Some(true), 1.5),
        (2, 2, "request", Some(false), 2.5),
        (3, 9, "other", None, 3.5),
    ] {
        sqlx::query(
            "INSERT INTO conditional_work_items
                (id, kind, ref_no, kind_label, kind_enabled, kind_score)
             VALUES (?, ?, 10, ?, ?, ?)",
        )
        .bind(id)
        .bind(kind)
        .bind(kind_label)
        .bind(kind_enabled)
        .bind(kind_score)
        .execute(&pool)
        .await?;
    }

    Ok(schema_builder(graphql_orm::db::Database::new(pool))
        .data("test-user".to_owned())
        .finish())
}

#[tokio::test]
async fn source_and_target_conditions_keep_polymorphic_relations_exact_and_batched()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = setup_schema().await?;
    graphql_orm::graphql::orm::reset_query_count();

    let response = schema
        .execute(
            r#"
            query {
              conditionalWorkItems(orderBy: [{ Id: ASC }]) {
                edges {
                  node {
                    Id
                    Kind
                    Job { Id Title }
                    Request { Id Title }
                    LabeledJob { Id }
                    EnabledJob { Id }
                    ScoredJob { Id }
                  }
                }
              }
              conditionalJobs {
                edges {
                  node {
                    Id
                    Work(orderBy: { Id: ASC }, page: { limit: 10 }) {
                      edges { node { Id Kind } }
                    }
                  }
                }
              }
              conditionalRequests {
                edges {
                  node {
                    Id
                    Work(orderBy: { Id: ASC }, page: { limit: 10 }) {
                      edges { node { Id Kind } }
                    }
                  }
                }
              }
            }
            "#,
        )
        .await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json()?;
    let work_items = data["conditionalWorkItems"]["edges"]
        .as_array()
        .expect("work item edges");
    assert!(work_items[0]["node"]["Job"].is_object());
    assert!(work_items[0]["node"]["Request"].is_null());
    assert!(work_items[0]["node"]["LabeledJob"].is_object());
    assert!(work_items[0]["node"]["EnabledJob"].is_object());
    assert!(work_items[0]["node"]["ScoredJob"].is_object());
    assert!(work_items[1]["node"]["Job"].is_null());
    assert!(work_items[1]["node"]["Request"].is_object());
    assert!(work_items[1]["node"]["LabeledJob"].is_null());
    assert!(work_items[1]["node"]["EnabledJob"].is_null());
    assert!(work_items[1]["node"]["ScoredJob"].is_null());
    assert!(work_items[2]["node"]["Job"].is_null());
    assert!(work_items[2]["node"]["Request"].is_null());
    assert!(work_items[2]["node"]["LabeledJob"].is_null());
    assert!(work_items[2]["node"]["EnabledJob"].is_null());
    assert!(work_items[2]["node"]["ScoredJob"].is_null());

    let job_work = data["conditionalJobs"]["edges"][0]["node"]["Work"]["edges"]
        .as_array()
        .expect("job work edges");
    assert_eq!(job_work.len(), 1);
    assert_eq!(job_work[0]["node"]["Kind"].as_i64(), Some(0));
    let request_work = data["conditionalRequests"]["edges"][0]["node"]["Work"]["edges"]
        .as_array()
        .expect("request work edges");
    assert_eq!(request_work.len(), 1);
    assert_eq!(request_work[0]["node"]["Kind"].as_i64(), Some(2));
    assert!(
        graphql_orm::graphql::orm::query_count() <= 12,
        "conditional relation expansion issued {} queries",
        graphql_orm::graphql::orm::query_count()
    );

    Ok(())
}

#[tokio::test]
async fn target_conditions_apply_to_argument_free_nested_bulk_preloads()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = setup_schema().await?;
    graphql_orm::graphql::orm::reset_query_count();

    let response = schema
        .execute(
            r#"
            query {
              conditionalJobs {
                edges { node { Work { edges { node { Id Kind } } } } }
              }
              conditionalRequests {
                edges { node { Work { edges { node { Id Kind } } } } }
              }
            }
            "#,
        )
        .await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json()?;
    let job_work = data["conditionalJobs"]["edges"][0]["node"]["Work"]["edges"]
        .as_array()
        .expect("job work edges");
    assert_eq!(job_work.len(), 1);
    assert_eq!(job_work[0]["node"]["Kind"].as_i64(), Some(0));
    let request_work = data["conditionalRequests"]["edges"][0]["node"]["Work"]["edges"]
        .as_array()
        .expect("request work edges");
    assert_eq!(request_work.len(), 1);
    assert_eq!(request_work[0]["node"]["Kind"].as_i64(), Some(2));
    assert!(
        graphql_orm::graphql::orm::query_count() <= 4,
        "conditional bulk preload issued {} queries",
        graphql_orm::graphql::orm::query_count()
    );

    Ok(())
}
