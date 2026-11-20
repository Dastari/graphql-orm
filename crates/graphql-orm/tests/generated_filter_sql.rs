// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

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
    let mut row_bound = String::new();
    // The row a read predicate renders the filter for: a table path or a
    // row alias. The filter writes it before each column; it is never
    // searched for in the SQL.
    let row = "the_row";
    for filter in [scalar, empty, nested] {
        let unqualified = filter.to_sql_conditions();
        actual.push_str(&format!(
            "{:?}\n{:?}\n",
            unqualified,
            filter.to_sql_prefilter_conditions(backend)
        ));
        let bound = filter.__gom_row_bound_filter().unwrap().conditions(row);
        row_bound.push_str(&format!("{bound:?}\n"));
        // The row-bound form, which read predicates and related-row
        // filters render (#796), differs only by naming the row and
        // delimiting each column.
        let plain = |conditions: &[String]| {
            conditions
                .iter()
                .map(|condition| {
                    condition
                        .replace(&format!("{row}."), "")
                        .replace(['"', '[', ']'], "")
                })
                .collect::<Vec<_>>()
        };
        assert!(bound.0.iter().any(|condition| condition.contains(row)));
        assert_eq!(plain(&bound.0), plain(&unqualified.0));
        assert_eq!(bound.1, unqualified.1);
    }
    pin("filter_sql", backend, &actual);
    pin("filter_sql_row_bound", backend, &row_bound);
}

fn pin(name: &str, backend: graphql_orm::graphql::orm::DatabaseBackend, actual: &str) {
    let path = format!(
        "{}/tests/snapshots/{name}_{backend:?}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var_os("PIN_FILTER_SQL").is_some() {
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
    }
    assert_eq!(actual, std::fs::read_to_string(path).unwrap());
}
