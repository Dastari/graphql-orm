use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.Jobs",
    plural = "Jobs",
    schema_policy = "external_read_only",
    default_sort = "JobId ASC"
)]
#[graphql(complex)]
pub struct Job {
    #[primary_key]
    #[graphql_orm(db_column = "JobId")]
    #[filterable(type = "number")]
    #[sortable]
    pub job_id: i32,

    #[graphql_orm(db_column = "JobName")]
    #[filterable(type = "string")]
    #[sortable]
    pub job_name: String,

    #[graphql_orm(db_column = "IsActive")]
    #[filterable(type = "bool")]
    pub is_active: bool,

    #[graphql(skip)]
    #[relation(target = "JobLabour", from = "job_id", to = "JobId", multiple, emit_fk = false)]
    pub labour_entries: Vec<JobLabour>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.JobLabour",
    plural = "JobLabourEntries",
    schema_policy = "external_read_only",
    default_sort = "LineNum ASC"
)]
#[graphql(complex)]
pub struct JobLabour {
    #[primary_key]
    #[graphql_orm(db_column = "LabourId")]
    #[filterable(type = "number")]
    #[sortable]
    pub labour_id: i32,

    #[graphql_orm(db_column = "JobId")]
    #[filterable(type = "number")]
    #[sortable]
    pub job_id: i32,

    #[graphql_orm(db_column = "LineNum")]
    #[filterable(type = "number")]
    #[sortable]
    pub line_num: i16,

    #[graphql_orm(db_column = "LabourDate")]
    #[filterable(type = "date")]
    #[sortable]
    pub labour_date: Option<String>,

    #[graphql(skip)]
    #[relation(target = "Job", from = "job_id", to = "JobId", emit_fk = false)]
    pub job: Option<Job>,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::MssqlBackend> for Job {
    fn batch_column() -> &'static str {
        "JobId"
    }

    fn batch_key_from_row(
        row: &graphql_orm::db::mssql::MssqlRow,
    ) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get::<i32, _>("JobId").map(|value| value.to_string())
    }
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::MssqlBackend> for JobLabour {
    fn batch_column() -> &'static str {
        "JobId"
    }

    fn batch_key_from_row(
        row: &graphql_orm::db::mssql::MssqlRow,
    ) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get::<i32, _>("JobId").map(|value| value.to_string())
    }
}

schema_roots! {
    backend: "mssql",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    entities: [Job, JobLabour],
}

pub fn build_schema(
    pool: graphql_orm::db::mssql::MssqlPool,
) -> graphql_orm::async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot> {
    schema_builder(graphql_orm::db::Database::<graphql_orm::MssqlBackend>::new(pool)).finish()
}
