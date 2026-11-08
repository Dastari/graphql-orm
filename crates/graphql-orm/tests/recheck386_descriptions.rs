// Recheck probe for PR #386: catalogue descriptions must equal the SDL
// descriptions macros 0.96 emit for every generated root field.
#![cfg(feature = "sqlite")]
#![cfg(not(any(feature = "postgres", feature = "mssql")))]

use graphql_orm::async_graphql::{ComplexObject, SimpleObject};
use graphql_orm::prelude::*;
use std::collections::BTreeMap;

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "desc_rows",
    plural = "DescRows",
    default_sort = "id ASC",
    aggregate = true,
    keyset = "label asc, id asc",
    auth = "none"
)]
struct DescRow {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(searchable(weight = "A"))]
    label: String,
}
#[ComplexObject]
impl DescRow {}

schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [DescRow], }

#[tokio::test]
async fn catalogue_descriptions_match_sdl() {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let schema = schema_builder(db).finish();
    let response = schema
        .execute("{ __schema { queryType { fields { name description } } mutationType { fields { name description } } subscriptionType { fields { name description } } } }")
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let mut sdl = BTreeMap::new();
    for root in ["queryType", "mutationType", "subscriptionType"] {
        if let Some(fields) = data["__schema"][root]["fields"].as_array() {
            for field in fields {
                sdl.insert(
                    field["name"].as_str().unwrap().to_owned(),
                    field["description"].as_str().map(str::to_owned),
                );
            }
        }
    }
    let catalog = graphql_orm_operation_catalog();
    let mut mismatches = Vec::new();
    for operation in catalog.operations() {
        let name = operation.field_name();
        let catalogue = operation.description();
        match sdl.get(name) {
            Some(Some(text)) if text.trim() == catalogue => {}
            other => {
                eprintln!("DIFF {name}: catalogue={catalogue:?} sdl={other:?}");
                mismatches.push(name.to_owned());
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "mismatched descriptions: {mismatches:?}"
    );
}
