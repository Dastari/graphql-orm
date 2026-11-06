//! Empty membership and boolean predicates must never silently widen SQL.
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "sql_filter_rows", auth = "none")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "sql_filter_rows", auth = "none")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "sql_filter_rows",
        schema_policy = "external_writable",
        auth = "none"
    )
)]
struct SqlFilterRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    text: String,
    #[filterable(type = "number")]
    number: i32,
    #[filterable(type = "int64")]
    wide: i64,
    #[filterable(type = "uuid")]
    uid: graphql_orm::uuid::Uuid,
    #[filterable(type = "bytes")]
    bytes: Vec<u8>,
    #[filterable(type = "boolean")]
    flag: Option<bool>,
}

#[test]
fn emitted_sql_and_bind_order_are_pinned() {
    let scalar = SqlFilterRowWhereInput {
        text: Some(StringFilter {
            eq: Some("a".into()),
            ne: Some("b".into()),
            contains: Some("%_".into()),
            starts_with: Some("c".into()),
            ends_with: Some("d".into()),
            in_list: Some(vec!["e".into(), "f".into()]),
            not_in: Some(vec!["g".into()]),
            is_null: Some(false),
            ..Default::default()
        }),
        number: Some(IntFilter {
            eq: Some(1),
            ne: Some(2),
            lt: Some(3),
            lte: Some(4),
            gt: Some(5),
            gte: Some(6),
            in_list: Some(vec![7, 8]),
            not_in: Some(vec![9]),
            is_null: Some(true),
        }),
        flag: Some(BoolFilter {
            eq: Some(true),
            ne: Some(false),
            is_null: Some(false),
        }),
        ..Default::default()
    };
    let empty = SqlFilterRowWhereInput {
        text: Some(StringFilter {
            in_list: Some(vec![]),
            not_in: Some(vec![]),
            is_null: Some(true),
            ..Default::default()
        }),
        number: Some(IntFilter {
            in_list: Some(vec![]),
            not_in: Some(vec![]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let nested = SqlFilterRowWhereInput {
        and: Some(vec![scalar.clone()]),
        or: Some(vec![empty.clone(), scalar.clone(), Default::default()]),
        not: Some(Box::new(scalar.clone())),
        ..Default::default()
    };
    let backend = graphql_orm::graphql::orm::current_backend();
    let mut actual = String::new();
    for filter in [scalar, empty, nested] {
        actual.push_str(&format!(
            "{:?}\n{:?}\n",
            filter.to_sql_conditions(),
            filter.to_sql_prefilter_conditions(backend)
        ));
    }
    let path = format!(
        "{}/tests/snapshots/filter_sql_{backend:?}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var_os("PIN_FILTER_SQL").is_some() {
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
    }
    assert_eq!(actual, std::fs::read_to_string(path).unwrap());
}
