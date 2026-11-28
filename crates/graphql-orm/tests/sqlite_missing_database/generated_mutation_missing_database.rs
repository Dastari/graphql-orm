#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::graphql::input_validation::GeneratedInputValidation;
use graphql_orm::prelude::*;
use sqlx::Row;

pub(super) const DATABASE_ERROR: &str = "graphql-orm Database runtime not registered; build the schema with schema_builder(database) or add Database to schema data";

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "records", auth = "none", upsert = "id")]
struct Record {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    title: String,
    #[graphql_orm(version = "managed", default = "0")]
    version: i64,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    async_graphql::SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_entity(table = "places", auth = "none")]
#[graphql(complex)]
struct Place {
    #[primary_key]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    parent_id: Option<String>,
    #[graphql(skip)]
    #[relation(target = "Place", from = "parent_id", to = "id")]
    parent: Option<Box<Place>>,
    #[graphql(skip)]
    #[relation(target = "Place", from = "id", to = "parent_id", multiple)]
    children: Vec<Place>,
}

impl BatchLoadEntity for Place {
    fn batch_column() -> &'static str {
        "id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("id")
    }
}

struct Query;

#[async_graphql::Object]
impl Query {
    async fn place(&self) -> Place {
        Place {
            id: "child".into(),
            parent_id: Some("parent".into()),
            parent: None,
            children: Vec::new(),
        }
    }

    async fn record(&self) -> Record {
        Record {
            id: "a".into(),
            title: "Record".into(),
            version: 0,
        }
    }
}

async fn assert_missing_database(document: &str, path: &[&str]) {
    let schema =
        async_graphql::Schema::build(Query, RecordMutations, async_graphql::EmptySubscription)
            .extension(GeneratedInputValidation)
            .finish();
    let response = schema.execute(document).await;
    assert_eq!(response.errors.len(), 1, "{document}: {response:?}");
    let error = &response.errors[0];
    assert_eq!(error.message, DATABASE_ERROR, "{document}: {error:?}");
    assert_eq!(
        error.path,
        path.iter()
            .map(|field| async_graphql::PathSegment::Field((*field).into()))
            .collect::<Vec<_>>(),
        "{document}: {error:?}",
    );
}

macro_rules! missing_database {
    ($test:ident, $root:literal, $arguments:literal) => {
        #[tokio::test]
        async fn $test() {
            assert_missing_database(
                concat!("mutation { ", $root, "(", $arguments, ") { success } }"),
                &[$root],
            )
            .await;
        }
    };
}

missing_database!(
    create_reports_missing_database,
    "createRecord",
    r#"input: {id: "a", title: "Record"}"#
);
missing_database!(
    upsert_reports_missing_database,
    "upsertRecord",
    r#"input: {id: "a", title: "Record"}"#
);
missing_database!(
    update_reports_missing_database,
    "updateRecord",
    r#"id: "a", input: {title: "Record"}"#
);
missing_database!(
    update_many_reports_missing_database,
    "updateRecords",
    r#"where: {title: {eq: "a"}}, input: {title: "Record"}"#
);
missing_database!(
    delete_reports_missing_database,
    "deleteRecord",
    r#"id: "a""#
);
missing_database!(
    delete_many_reports_missing_database,
    "deleteRecords",
    r#"where: {title: {eq: "a"}}"#
);

#[tokio::test]
async fn scalar_getter_reports_missing_database() {
    assert_missing_database("{ record { title } }", &["record", "title"]).await;
}

#[tokio::test]
async fn record_version_getter_reports_missing_database() {
    assert_missing_database("{ record { RecordVersion } }", &["record", "RecordVersion"]).await;
}

#[tokio::test]
async fn single_relation_reports_missing_database() {
    assert_missing_database("{ place { parent { id } } }", &["place", "parent"]).await;
}

#[tokio::test]
async fn list_relation_reports_missing_database() {
    assert_missing_database(
        "{ place { children { edges { node { id } } } } }",
        &["place", "children"],
    )
    .await;
}
