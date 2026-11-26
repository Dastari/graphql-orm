#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::graphql::input_validation::GeneratedInputValidation;
use graphql_orm::graphql::subscriptions::GeneratedSubscriptionAdmission;
use graphql_orm::prelude::*;

const DATABASE_ERROR: &str = "graphql-orm Database runtime not registered; build the schema with schema_builder(database) or add Database to schema data";
const LOADER_ERROR: &str =
    "graphql-orm Federation loader not registered; build the schema with schema_builder(database)";

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "missing_database_records",
    plural = "MissingDatabaseRecords",
    auth = "none",
    aggregate = true,
    keyset = "id asc",
    federation_key(fields = ["id"]),
    node_id
)]
struct MissingDatabaseRecord {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(searchable(weight = "A"))]
    title: String,
}

async fn assert_missing_database(field: &str) {
    let schema = async_graphql::Schema::build(
        MissingDatabaseRecordQueries,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .extension(GeneratedInputValidation)
    .finish();

    let response = schema.execute(format!("{{ {field} }}")).await;
    assert_eq!(response.errors.len(), 1, "{field}: {response:?}");
    let error = &response.errors[0];
    assert_eq!(error.message, DATABASE_ERROR, "{field}: {error:?}");
    assert_eq!(error.path.len(), 1, "{field}: {error:?}");
}

#[tokio::test]
async fn point_read_returns_a_missing_database_error() {
    assert_missing_database(r#"missingDatabaseRecord(id: "a") { id }"#).await;
}

#[tokio::test]
async fn list_returns_a_missing_database_error() {
    assert_missing_database("missingDatabaseRecords { edges { cursor } }").await;
}

#[tokio::test]
async fn keyset_and_scan_return_missing_database_errors() {
    assert_missing_database("missingDatabaseRecordsKeyset(page: {limit: 1}) { edges { cursor } }")
        .await;
    assert_missing_database("missingDatabaseRecordsScan(page: {limit: 1}) { pageInfo { status } }")
        .await;
}

#[tokio::test]
async fn search_returns_a_missing_database_error() {
    assert_missing_database(
        r#"missingDatabaseRecordsSearch(search: {query: "a"}) { edges { cursor } }"#,
    )
    .await;
}

#[tokio::test]
async fn aggregate_returns_a_missing_database_error() {
    assert_missing_database("missingDatabaseRecordsAggregate(groupBy: [], metrics: [{operator: COUNT}], groupLimit: 1) { metrics { value } }").await;
}

#[tokio::test]
async fn federation_reads_report_missing_database_and_loader() {
    let id = graphql_orm::graphql::federation::encode_node_id(
        "MissingDatabaseRecord",
        vec![serde_json::json!("a")],
    );
    let queries = [
        r#"{ _entities(representations: [{__typename: "MissingDatabaseRecord", id: "a"}]) { ... on MissingDatabaseRecord { id } } }"#.to_owned(),
        format!(r#"{{ _entities(representations: [{{__typename: "MissingDatabaseRecord", NodeId: "{}"}}]) {{ ... on MissingDatabaseRecord {{ id }} }} }}"#, id.0),
    ];
    for register_database in [false, true] {
        let mut builder = async_graphql::Schema::build(
            MissingDatabaseRecordQueries,
            async_graphql::EmptyMutation,
            async_graphql::EmptySubscription,
        )
        .enable_federation();
        if register_database {
            builder = builder.data(Database::connect_sqlite("sqlite::memory:").await.unwrap());
        }
        let schema = builder.finish();
        let expected = if register_database {
            LOADER_ERROR
        } else {
            DATABASE_ERROR
        };
        for query in &queries {
            let response = schema.execute(query).await;
            assert_eq!(response.errors.len(), 1, "{query}: {response:?}");
            assert_eq!(
                response.errors[0].message, expected,
                "{query}: {response:?}"
            );
        }
    }
}

#[tokio::test]
async fn changed_subscription_uses_the_same_curated_database_error() {
    use graphql_orm::futures::StreamExt;

    let schema = async_graphql::Schema::build(
        MissingDatabaseRecordQueries,
        async_graphql::EmptyMutation,
        MissingDatabaseRecordSubscriptions,
    )
    .extension(GeneratedInputValidation)
    .extension(GeneratedSubscriptionAdmission(vec![
        MissingDatabaseRecord::__GOM_CHANGED_ROOT_NAME,
    ]))
    .finish();
    let mut stream =
        schema.execute_stream("subscription { missingDatabaseRecordChanged { action } }");
    let response = stream.next().await.expect("missing database response");
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(response.errors[0].message, DATABASE_ERROR);
}

struct DirectRecordQuery;

#[async_graphql::Object]
impl DirectRecordQuery {
    async fn record(&self) -> MissingDatabaseRecord {
        MissingDatabaseRecord {
            id: "a".into(),
            title: "Record".into(),
        }
    }
}

#[tokio::test]
async fn node_id_object_field_reports_the_curated_database_error() {
    let schema = async_graphql::Schema::build(
        DirectRecordQuery,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .finish();
    let response = schema.execute("{ record { NodeId } }").await;
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(response.errors[0].message, DATABASE_ERROR);
}

#[derive(GraphQLEntity, GraphQLRelations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "relation_sources",
    auth = "none",
    schema_only = true
)]
struct RelationSource {
    #[primary_key]
    id: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "MissingDatabaseRecord",
        from = "id",
        to = "id",
        emit_fk = false
    )]
    record: Option<MissingDatabaseRecord>,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(
        target = "MissingDatabaseRecord",
        from = "id",
        to = "id",
        multiple,
        emit_fk = false
    )]
    records: Vec<MissingDatabaseRecord>,
}

struct DirectRelationQuery;

#[async_graphql::Object]
impl DirectRelationQuery {
    async fn source(&self) -> RelationSource {
        RelationSource {
            id: "a".into(),
            record: None,
            records: Vec::new(),
        }
    }
}

async fn assert_missing_relation_loader(field: &str) {
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE missing_database_records (id TEXT PRIMARY KEY, title TEXT NOT NULL)")
        .execute(db.pool())
        .await
        .unwrap();
    let schema = async_graphql::Schema::build(
        DirectRelationQuery,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .extension(GeneratedInputValidation)
    .data(db)
    .finish();

    let response = schema.execute(format!("{{ source {{ {field} }} }}")).await;
    assert_eq!(response.errors.len(), 1, "{field}: {response:?}");
    assert_eq!(
        response.errors[0].message,
        "graphql-orm Relation loader not registered; build the schema with schema_builder(database)",
        "{field}: {response:?}",
    );
    assert_eq!(response.errors[0].path.len(), 2, "{field}: {response:?}");
}

#[tokio::test]
async fn single_relation_reports_a_missing_loader() {
    assert_missing_relation_loader("record { id }").await;
}

#[tokio::test]
async fn multiple_relation_reports_a_missing_loader() {
    assert_missing_relation_loader("records { edges { node { id } } }").await;
}
