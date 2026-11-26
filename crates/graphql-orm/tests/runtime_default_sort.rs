#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
))]

use graphql_orm::graphql::orm::{
    Entity, FieldId, RuntimeOrderDirection, RuntimeOrderTerm, RuntimeSchema,
};
use graphql_orm_macros::GraphQLSchemaEntity;

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    not(feature = "mssql"),
    graphql_entity(
        table = "sort_records",
        default_sort = r#"sort_records.id DESC, "line,no" ASC, "order""#
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        table = "sort_records",
        default_sort = "sort_records.id DESC, [line,no] ASC, [order]"
    )
)]
struct SortRecord {
    #[primary_key]
    id: String,
    #[graphql_orm(db_column = "line,no")]
    line_number: i64,
    #[graphql_orm(db_column = "order")]
    position: i64,
}

#[test]
fn runtime_default_order_resolves_qualified_and_delimited_columns() {
    let entity = SortRecord::metadata();
    let schema = RuntimeSchema::from_static_entities(&[entity]).unwrap();
    let table = entity.table_name;
    let collection = &schema.collections[0];
    let expected = [
        ("id", RuntimeOrderDirection::Desc),
        ("line,no", RuntimeOrderDirection::Asc),
        ("order", RuntimeOrderDirection::Asc),
    ]
    .map(|(column, direction)| RuntimeOrderTerm {
        field: FieldId::new(format!("{table}.{column}")).unwrap(),
        direction,
    });
    assert_eq!(collection.default_order, expected);
    for term in &collection.default_order {
        assert!(collection.fields.iter().any(|field| field.id == term.field));
    }
}

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "composite_sort_records")]
struct CompositeSortRecord {
    #[primary_key]
    #[graphql_orm(db_column = "order")]
    position: i64,
    #[primary_key]
    id: String,
}

#[test]
fn runtime_default_order_uses_declared_primary_key_names() {
    let entity = CompositeSortRecord::metadata();
    let schema = RuntimeSchema::from_static_entities(&[entity]).unwrap();
    let table = entity.table_name;
    let terms = &schema.collections[0].default_order;
    assert_eq!(
        terms,
        &["order", "id"].map(|column| RuntimeOrderTerm {
            field: FieldId::new(format!("{table}.{column}")).unwrap(),
            direction: RuntimeOrderDirection::Asc,
        })
    );
}
