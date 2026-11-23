// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! The root field names `schema_roots!` checks are the names async-graphql
//! serves, including where digits meet letters, and a declaration that omits
//! a served field is refused when the schema is built (#282).

use graphql_orm::async_graphql::{EmptyMutation, EmptySubscription, MergedObject, Object, Schema};
use graphql_orm::graphql::orm::{GraphqlCustomOperationMetadata, GraphqlRootFieldNames};
use graphql_orm::prelude::*;
use std::collections::BTreeSet;

macro_rules! digit_root {
    ($name:ident $(, $rule:literal)?) => {
        #[derive(Default)]
        struct $name;

        #[graphql_orm_root_fields]
        #[Object$((rename_fields = $rule))?]
        impl $name {
            async fn s3bucket(&self) -> i32 {
                0
            }
            async fn http2_status(&self) -> i32 {
                0
            }
            async fn get_url_v2(&self) -> i32 {
                0
            }
            async fn x_y_z1(&self) -> i32 {
                0
            }
            async fn item9_count_10x(&self) -> i32 {
                0
            }
            #[graphql(name = "Explicit_Name")]
            async fn renamed(&self) -> i32 {
                0
            }
            #[graphql(skip)]
            #[allow(dead_code)]
            async fn skipped(&self) -> i32 {
                0
            }
        }
    };
}
digit_root!(DefaultCase);
digit_root!(CamelCase, "camelCase");
digit_root!(PascalCase, "PascalCase");
digit_root!(SnakeCase, "snake_case");
digit_root!(ScreamingCase, "SCREAMING_SNAKE_CASE");
digit_root!(LowerCase, "lowercase");
digit_root!(UpperCase, "UPPERCASE");

#[derive(Default)]
struct DescribedDigits;

#[graphql_orm_custom_operations(kind = "query", authorization = false)]
#[Object]
impl DescribedDigits {
    /// Returns the status of one bucket.
    async fn s3bucket_status(&self, max_v2items: i32, page2_size: i32) -> i32 {
        max_v2items + page2_size
    }

    /// Returns the status of an HTTP/2 stream.
    async fn http2_status(&self, stream_id2: i32) -> i32 {
        stream_id2
    }

    /// Returns the second version of a link.
    async fn get_url_v2(&self, x_y_z1: i32) -> i32 {
        x_y_z1
    }

    /// Counts items in a numbered group.
    async fn item9_count_10x(&self, r#type: i32) -> i32 {
        r#type
    }
}

/// Identifiers on which `convert_case` and Inflector split differently (#576).
const DISAGREEING_IDENTIFIERS: [&str; 4] =
    ["s3BucketStatus", "http2Status", "getUrlV2", "item9Count10X"];

/// The fields and argument names async-graphql serves for a query root.
async fn served<T>(root: T, type_name: &str) -> Vec<(String, Vec<String>)>
where
    T: graphql_orm::async_graphql::ObjectType + 'static,
{
    let schema = Schema::build(root, EmptyMutation, EmptySubscription).finish();
    let response = schema
        .execute(format!(
            "{{ __type(name: \"{type_name}\") {{ fields {{ name args {{ name }} }} }} }}"
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()["__type"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| {
            (
                field["name"].as_str().unwrap().to_owned(),
                field["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|argument| argument["name"].as_str().unwrap().to_owned())
                    .collect(),
            )
        })
        .collect()
}

fn names(fields: &[&str]) -> BTreeSet<String> {
    fields.iter().map(|field| (*field).to_owned()).collect()
}

macro_rules! assert_declared_names_are_served_names {
    ($($root:ident),*) => {$(
        let fields = served($root, stringify!($root)).await;
        assert_eq!(
            names(<$root as GraphqlRootFieldNames>::ROOT_FIELD_NAMES),
            fields.iter().map(|(name, _)| name.clone()).collect(),
            stringify!($root)
        );
    )*};
}

#[tokio::test]
async fn declared_root_field_names_equal_the_served_names_for_every_rename_rule() {
    assert_declared_names_are_served_names!(
        DefaultCase,
        CamelCase,
        PascalCase,
        SnakeCase,
        ScreamingCase,
        LowerCase,
        UpperCase
    );
}

#[tokio::test]
async fn described_roots_catalogue_the_served_field_and_argument_names() {
    let served = served(DescribedDigits, "DescribedDigits").await;
    assert_eq!(
        names(DescribedDigits::ROOT_FIELD_NAMES),
        served.iter().map(|(name, _)| name.clone()).collect()
    );
    let catalogued = DescribedDigits::graphql_custom_operations()
        .iter()
        .map(|operation| {
            (
                operation.field_name.clone(),
                operation
                    .arguments
                    .iter()
                    .map(|argument| argument.graphql_name.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(catalogued, served);
    // The fixture really covers names where digits meet letters: each is
    // served and catalogued under the same coordinate.
    for field in DISAGREEING_IDENTIFIERS {
        assert!(
            served.iter().any(|(name, _)| name == field),
            "{field} is not served: {served:?}"
        );
    }
}

mod merged {
    use super::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(backend = "sqlite", table = "merged_things", plural = "MergedThings")]
    pub struct MergedThing {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
    }

    #[derive(Default)]
    pub struct StatusPart;
    #[graphql_orm_root_fields]
    #[Object]
    impl StatusPart {
        async fn status(&self) -> i32 {
            0
        }
    }

    #[derive(Default)]
    pub struct ReportPart;
    #[graphql_orm_root_fields]
    #[Object]
    impl ReportPart {
        async fn report(&self) -> i32 {
            0
        }
    }

    /// A merged root declares the fields of every member by hand.
    #[derive(MergedObject, Default)]
    pub struct CompleteRoot(StatusPart, ReportPart);
    impl GraphqlRootFieldNames for CompleteRoot {
        const ROOT_FIELD_NAMES: &'static [&'static str] = &["status", "report"];
    }

    /// A manual declaration that leaves a member's field out.
    #[derive(MergedObject, Default)]
    pub struct IncompleteRoot(StatusPart, ReportPart);
    impl GraphqlRootFieldNames for IncompleteRoot {
        const ROOT_FIELD_NAMES: &'static [&'static str] = &["status"];
    }

    pub mod complete {
        use super::*;
        schema_roots! {
            backend: "sqlite",
            query_custom_ops: [],
            entities: [MergedThing],
            extra_query_types: [CompleteRoot],
        }
    }
    pub mod incomplete {
        use super::*;
        schema_roots! {
            backend: "sqlite",
            query_custom_ops: [],
            entities: [MergedThing],
            extra_query_types: [IncompleteRoot],
        }
    }
}

#[tokio::test]
async fn a_merged_root_with_a_complete_manual_declaration_builds() {
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let schema = merged::complete::schema_builder(db).finish();
    let response = schema
        .execute(
            graphql_orm::async_graphql::Request::new("{ status report }")
                .data(AuthSubject::new("fixture-principal")),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
}

#[tokio::test]
#[should_panic(
    expected = "`IncompleteRoot` exposes root field(s) `report` that its `GraphqlRootFieldNames` implementation does not declare"
)]
async fn a_manual_declaration_that_omits_a_served_field_is_refused_at_schema_build() {
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let _ = merged::incomplete::schema_builder(db);
}
