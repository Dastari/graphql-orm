//! Missing-table failures on additional generated roots must remain sanitized.
#![cfg(feature = "sqlite")]
#![cfg(not(any(feature = "postgres", feature = "mssql")))]

use graphql_orm::async_graphql::{Request, SimpleObject};
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "probe_private_missing",
    plural = "ProbeMissings",
    default_sort = "id ASC",
    aggregate = true,
    keyset = "id asc",
    node_id,
    auth = "none"
)]
struct ProbeMissing {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
    #[filterable(type = "number")]
    units: i64,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [ProbeMissing],
}

#[tokio::test]
async fn missing_table_on_other_generated_roots() {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let schema = schema_builder(db).finish();
    let node_id = graphql_orm::graphql::federation::encode_node_id(
        "ProbeMissing",
        vec![serde_json::json!("a")],
    )
    .to_string();
    let queries = vec![
        "{ probeMissingsAggregate(groupBy: [label], metrics: [{operator: COUNT}], groupLimit: 10) { groups { value } metrics { value } } }".to_string(),
        "{ probeMissingsKeyset(page: { limit: 2 }) { edges { node { id } } } }".to_string(),
        "{ probeMissings(where: {units: {gt: 1}}, orderBy: {label: ASC}) { edges { node { id } } pageInfo { totalCount } } }".to_string(),
        format!("{{ _entities(representations: [{{__typename: \"ProbeMissing\", NodeId: \"{node_id}\"}}]) {{ ... on ProbeMissing {{ id }} }} }}"),
        "mutation { updateProbeMissings(where: {id: {eq: \"a\"}}, input: {label: \"b\"}) { success error affectedCount } }".to_string(),
        "mutation { deleteProbeMissings(where: {id: {eq: \"a\"}}) { success error deletedCount } }".to_string(),
    ];
    let mut leaks = Vec::new();
    for query in queries {
        let response = schema
            .execute(Request::new(query.clone()).data(AuthSubject::new("p")))
            .await;
        if response.errors.is_empty() {
            assert!(
                query.starts_with("mutation"),
                "missing-table query must fail: {query}"
            );
            let value = serde_json::to_value(&response).unwrap();
            let result = value["data"]
                .as_object()
                .expect("valid mutation response")
                .values()
                .next()
                .unwrap();
            assert_eq!(result["success"], false, "{query}: {value}");
            assert_eq!(result["error"], "internal error", "{query}: {value}");
        } else {
            for error in &response.errors {
                assert_eq!(error.message, "internal error", "{query}");
                assert_eq!(
                    error
                        .extensions
                        .as_ref()
                        .and_then(|extensions| extensions.get("code")),
                    Some(&graphql_orm::async_graphql::Value::String(
                        "INTERNAL_ERROR".into()
                    )),
                    "{query}"
                );
            }
        }
        let rendered = serde_json::to_string(&response).unwrap();
        if rendered.contains("probe_private_missing") || rendered.contains("no such table") {
            leaks.push(rendered);
        }
    }
    assert!(
        leaks.is_empty(),
        "raw SQLx text reached clients: {leaks:#?}"
    );
}
