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
#[graphql(rename_fields = "PascalCase")]
#[graphql(complex)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.JimCardFile",
    plural = "JimCardFiles",
    schema_policy = "external_read_only",
    default_sort = "[CardNo] ASC"
)]
pub struct JimCardFile {
    #[primary_key]
    #[graphql(name = "CardNo")]
    #[graphql_orm(db_column = "CardNo", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub card_no: i32,

    #[graphql(name = "CardCode")]
    #[graphql_orm(db_column = "CardCode", write = false)]
    #[filterable(type = "string")]
    pub card_code: String,

    #[graphql(name = "Name")]
    #[graphql_orm(db_column = "Name", write = false)]
    #[filterable(type = "string")]
    pub name: Option<String>,

    #[graphql(skip, name = "Contacts")]
    #[relation(
        target = "JimCardFileContact",
        from = "card_no",
        to = "CardNo",
        multiple,
        emit_fk = false
    )]
    pub contacts: Vec<JimCardFileContact>,
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
#[graphql(rename_fields = "PascalCase")]
#[graphql(complex)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.JimCardFileContacts",
    plural = "JimCardFileContacts",
    schema_policy = "external_read_only",
    default_sort = "[CardNo] ASC, [ContNo] ASC"
)]
pub struct JimCardFileContact {
    #[primary_key]
    #[graphql(name = "CardNo")]
    #[graphql_orm(db_column = "CardNo", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub card_no: i32,

    #[primary_key]
    #[graphql(name = "ContNo")]
    #[graphql_orm(db_column = "ContNo", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub cont_no: i32,

    #[graphql(name = "DName")]
    #[graphql_orm(db_column = "DName", write = false)]
    #[filterable(type = "string")]
    pub display_name: Option<String>,

    #[graphql(name = "JobTitle")]
    #[graphql_orm(db_column = "JobTitle", write = false)]
    #[filterable(type = "string")]
    pub job_title: Option<String>,

    #[graphql(skip, name = "Details")]
    #[relation(
        target = "JimCardFileDetail",
        from = ["card_no", "cont_no"],
        to = ["CardNo", "ContNo"],
        multiple,
        emit_fk = false
    )]
    pub details: Vec<JimCardFileDetail>,
}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.JimCardFileDetails",
    plural = "JimCardFileDetails",
    schema_policy = "external_read_only",
    default_sort = "[CardNo] ASC, [ContNo] ASC, [LineNum] ASC"
)]
pub struct JimCardFileDetail {
    #[primary_key]
    #[graphql(name = "CardNo")]
    #[graphql_orm(db_column = "CardNo", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub card_no: i32,

    #[primary_key]
    #[graphql(name = "ContNo")]
    #[graphql_orm(db_column = "ContNo", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub cont_no: i32,

    #[primary_key]
    #[graphql(name = "LineNum")]
    #[graphql_orm(db_column = "LineNum", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub line_num: i16,

    #[graphql(name = "Type")]
    #[graphql_orm(db_column = "Type", write = false)]
    #[filterable(type = "string")]
    pub detail_type: Option<String>,

    #[graphql(name = "Value")]
    #[graphql_orm(db_column = "Value", write = false)]
    #[filterable(type = "string")]
    pub value: Option<String>,

    #[graphql(name = "Comments")]
    #[graphql_orm(db_column = "Comments", write = false)]
    #[filterable(type = "string")]
    pub comments: Option<String>,
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

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::MssqlBackend> for JimCardFileContact {
    fn batch_column() -> &'static str {
        "CardNo"
    }

    fn batch_key_from_row(
        row: &graphql_orm::db::mssql::MssqlRow,
    ) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get::<i32, _>("CardNo").map(|value| value.to_string())
    }
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::MssqlBackend> for JimCardFileDetail {
    fn batch_column() -> &'static str {
        "CardNo"
    }

    fn batch_key_from_row(
        row: &graphql_orm::db::mssql::MssqlRow,
    ) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get::<i32, _>("CardNo").map(|value| value.to_string())
    }
}

schema_roots! {
    backend: "mssql",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    entities: [Job, JobLabour, JimCardFile, JimCardFileContact, JimCardFileDetail],
}

pub fn build_schema(
    pool: graphql_orm::db::mssql::MssqlPool,
) -> graphql_orm::async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot> {
    schema_builder(graphql_orm::db::Database::<graphql_orm::MssqlBackend>::new(pool)).finish()
}
