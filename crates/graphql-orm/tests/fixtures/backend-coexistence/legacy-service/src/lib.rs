use graphql_orm::async_graphql::{Object, SimpleObject};
use graphql_orm::prelude::*;
use graphql_orm_ai_tool_profiles::{
    AiDisclosureRule, AiDisclosureSchema, AiDisclosureShape, AiGeneratedGraphqlOperationPolicy,
    AiGraphqlArgumentPlan, AiGraphqlArgumentValue, AiGraphqlProfileInput, AiGraphqlSelection,
    AiGraphqlToolManifest, AiGraphqlToolManifestBuilder, AiGraphqlToolProfile, DataClassification,
    GraphqlExecutionTargetId,
};

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
    #[relation(
        target = "JobLabour",
        from = "job_id",
        to = "JobId",
        multiple,
        emit_fk = false
    )]
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
    table = "dbo.LegacyCardFile",
    plural = "LegacyCardFiles",
    schema_policy = "external_read_only",
    default_sort = "[CardNo] ASC"
)]
pub struct LegacyCardFile {
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
        target = "LegacyCardFileContact",
        from = "card_no",
        to = "CardNo",
        multiple,
        emit_fk = false
    )]
    pub contacts: Vec<LegacyCardFileContact>,
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
    table = "dbo.LegacyCardFileContacts",
    plural = "LegacyCardFileContacts",
    schema_policy = "external_read_only",
    default_sort = "[CardNo] ASC, [ContNo] ASC"
)]
pub struct LegacyCardFileContact {
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
        target = "LegacyCardFileDetail",
        from = ["card_no", "cont_no"],
        to = ["CardNo", "ContNo"],
        multiple,
        emit_fk = false
    )]
    pub details: Vec<LegacyCardFileDetail>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.LegacyCardFileDetails",
    plural = "LegacyCardFileDetails",
    schema_policy = "external_read_only",
    default_sort = "[CardNo] ASC, [ContNo] ASC, [LineNum] ASC"
)]
pub struct LegacyCardFileDetail {
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
        row.try_get::<i32, _>("JobId")
            .map(|value| value.to_string())
    }
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::MssqlBackend> for JobLabour {
    fn batch_column() -> &'static str {
        "JobId"
    }

    fn batch_key_from_row(
        row: &graphql_orm::db::mssql::MssqlRow,
    ) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get::<i32, _>("JobId")
            .map(|value| value.to_string())
    }
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::MssqlBackend>
    for LegacyCardFileContact
{
    fn batch_column() -> &'static str {
        "CardNo"
    }

    fn batch_key_from_row(
        row: &graphql_orm::db::mssql::MssqlRow,
    ) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get::<i32, _>("CardNo")
            .map(|value| value.to_string())
    }
}

impl graphql_orm::graphql::loaders::BatchLoadEntity<graphql_orm::MssqlBackend>
    for LegacyCardFileDetail
{
    fn batch_column() -> &'static str {
        "CardNo"
    }

    fn batch_key_from_row(
        row: &graphql_orm::db::mssql::MssqlRow,
    ) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get::<i32, _>("CardNo")
            .map(|value| value.to_string())
    }
}

#[derive(Clone, Debug, SimpleObject)]
#[graphql(rename_fields = "PascalCase")]
pub struct LegacyComment {
    pub line_no: i32,
    pub comment: String,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LegacyCustomQuery;

#[Object(rename_fields = "PascalCase", rename_args = "PascalCase")]
impl LegacyCustomQuery {
    async fn legacy_work_item_comments(&self, job_no: String, first: i32) -> Vec<LegacyComment> {
        let _ = (job_no, first);
        Vec::new()
    }
}

schema_roots! {
    backend: "mssql",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    extra_query_types: [LegacyCustomQuery],
    entities: [Job, JobLabour, LegacyCardFile, LegacyCardFileContact, LegacyCardFileDetail],
}

pub fn build_schema(
    pool: graphql_orm::db::mssql::MssqlPool,
) -> graphql_orm::async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot> {
    schema_builder(graphql_orm::db::Database::<graphql_orm::MssqlBackend>::new(
        pool,
    ))
    .finish()
}

struct AdmitLegacyGenerated;

impl AiGeneratedGraphqlOperationPolicy for AdmitLegacyGenerated {
    fn is_application_operation(&self, operation: &GraphqlResolverOperationDescriptor) -> bool {
        operation.entity_name() == "Job"
    }
}

fn disclosure(version: &str, root: &str, root_shape: AiDisclosureShape) -> AiDisclosureSchema {
    let rule = AiDisclosureRule::exportable(DataClassification::Confidential);
    AiDisclosureSchema::new(
        version,
        AiDisclosureShape::object(rule, [(root.to_owned(), root_shape)]),
    )
    .expect("fixture disclosure is valid")
}

pub fn ai_tool_manifest() -> Result<(String, AiGraphqlToolManifest), String> {
    let mut config = graphql_orm::tiberius::Config::new();
    config.host("fixture.invalid");
    config.port(1433);
    config.authentication(graphql_orm::tiberius::AuthMethod::sql_server(
        "fixture", "fixture",
    ));
    let pool = graphql_orm::db::mssql::MssqlPool::new(config);
    let sdl = build_schema(pool).sdl();
    let catalog = graphql_orm_operation_catalog();
    let operation = catalog
        .exposed_operations()
        .find(|operation| {
            operation.entity_name() == "Job"
                && operation.category() == GeneratedGraphqlOperationCategory::SingleRead
        })
        .ok_or_else(|| "missing generated Job detail operation".to_owned())?;
    let id_argument = operation
        .arguments()
        .first()
        .ok_or_else(|| "missing generated Job identity argument".to_owned())?;
    let rule = AiDisclosureRule::exportable(DataClassification::Confidential);
    let generated = AiGraphqlToolProfile::read_only(
        "details",
        operation.field_name(),
        "Show a reviewed subset of one visible Legacy job",
        vec![
            AiGraphqlSelection::scalar("jobId"),
            AiGraphqlSelection::scalar("jobName"),
        ],
        disclosure(
            "legacy-job-details-v1",
            operation.field_name(),
            AiDisclosureShape::object(
                rule,
                [
                    ("jobId".to_owned(), AiDisclosureShape::scalar(rule)),
                    ("jobName".to_owned(), AiDisclosureShape::scalar(rule)),
                ],
            ),
        ),
        16 * 1024,
        1,
    )
    .with_inputs([AiGraphqlProfileInput::integer(
        "JobNo",
        "Public Legacy job number",
        true,
        1,
        i64::from(i32::MAX),
    )])
    .with_arguments([AiGraphqlArgumentPlan::new(
        id_argument.graphql_name(),
        AiGraphqlArgumentValue::input("JobNo"),
    )]);
    let custom = AiGraphqlToolProfile::read_only(
        "comments",
        "LegacyWorkItemComments",
        "List a bounded reviewed set of comments for one Legacy work item",
        vec![
            AiGraphqlSelection::scalar("LineNo"),
            AiGraphqlSelection::scalar("Comment"),
        ],
        disclosure(
            "legacy-comments-v1",
            "LegacyWorkItemComments",
            AiDisclosureShape::list(
                rule,
                25,
                AiDisclosureShape::object(
                    rule,
                    [
                        ("LineNo".to_owned(), AiDisclosureShape::scalar(rule)),
                        ("Comment".to_owned(), AiDisclosureShape::scalar(rule)),
                    ],
                ),
            ),
        ),
        32 * 1024,
        25,
    )
    .with_root_list_bound(25)
    .with_inputs([
        AiGraphqlProfileInput::string("JobNo", "Public Legacy job number", true, 1, 64),
        AiGraphqlProfileInput::integer("Limit", "Maximum comment count", true, 1, 25),
    ])
    .with_arguments([
        AiGraphqlArgumentPlan::new("JobNo", AiGraphqlArgumentValue::input("JobNo")),
        AiGraphqlArgumentPlan::new("First", AiGraphqlArgumentValue::input("Limit")),
    ]);

    let target =
        GraphqlExecutionTargetId::parse("legacy-graph").map_err(|error| error.to_string())?;
    let mut builder = AiGraphqlToolManifestBuilder::new("legacy-service", target, &sdl)
        .map_err(|error| error.to_string())?;
    builder
        .add_generated_profile(generated, catalog, &AdmitLegacyGenerated)
        .map_err(|error| error.to_string())?;
    builder
        .add_custom_profile(custom)
        .map_err(|error| error.to_string())?;
    let manifest = builder.build().map_err(|error| error.to_string())?;
    Ok((sdl, manifest))
}
