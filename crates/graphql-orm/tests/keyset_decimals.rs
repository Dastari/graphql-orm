// This fixture selects one backend per build.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::graphql::filters::DecimalFilter;
use graphql_orm::graphql::orm::GraphqlGeneratedInputKind;
use graphql_orm::graphql::pagination::{KeysetValue, decode_keyset_cursor, encode_keyset_cursor};
use graphql_orm::prelude::*;
use graphql_orm::rust_decimal::Decimal;
use serde_json::{Value, json};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "keyset_amounts",
        plural = "KeysetAmounts",
        keyset = "id asc",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "keyset_amounts",
        plural = "KeysetAmounts",
        keyset = "id asc",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "keyset_amounts",
        plural = "KeysetAmounts",
        keyset = "id asc",
        auth = "none",
        schema_policy = "external_writable"
    )
)]
struct KeysetAmount {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[graphql_orm(
        decimal(precision = 18, scale = 2),
        db_column = "stored_amount",
        non_exportable
    )]
    #[filterable(type = "decimal")]
    #[sortable]
    amount: Option<Decimal>,
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [KeysetAmount] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [KeysetAmount] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [KeysetAmount] }

fn assert_schema_and_catalog(db: &Database) {
    let sdl = schema_builder(db.clone()).finish().sdl();
    let (_, input) = sdl
        .split_once("input KeysetAmountKeysetOrderByInput {")
        .unwrap();
    assert!(
        input
            .split_once('}')
            .unwrap()
            .0
            .contains("amount: OrderDirection")
    );

    let catalog = graphql_orm_semantic_catalog();
    let entity = catalog
        .entities
        .iter()
        .find(|entity| entity.entity_name == "KeysetAmount")
        .unwrap();
    assert!(entity.generated_input_members.iter().any(|member| {
        member.kind == GraphqlGeneratedInputKind::KeysetOrderBy && member.field_name == "amount"
    }));
}

const ROWS: [(&str, Option<&str>); 8] = [
    ("c", Some("-9999999999999999.99")),
    ("f", Some("-0.01")),
    ("b", Some("0.00")),
    ("d", Some("9999999999999999.98")),
    ("a", Some("9999999999999999.98")),
    ("g", Some("9999999999999999.99")),
    ("z", None),
    ("h", None),
];

fn order(direction: OrderDirection) -> Option<Vec<KeysetAmountKeysetOrderByInput>> {
    Some(
        KeysetAmountKeysetOrderByInput::ordered([(KeysetAmountOrderField::Amount, direction)])
            .unwrap(),
    )
}

async fn page(
    db: &Database,
    direction: OrderDirection,
    after: Option<String>,
) -> async_graphql::Result<Connection<KeysetAmount>> {
    KeysetAmount::keyset_page_ordered(
        db,
        Default::default(),
        order(direction),
        KeysetPageInput {
            after,
            // One edge per page crosses both identical values and identical NULLs.
            limit: Some(1),
            include_total_count: false,
        },
    )
    .await
}

fn cursor_envelope(cursor: &str) -> Value {
    let hex = cursor.strip_prefix("gomk1.").unwrap();
    let bytes: Vec<_> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    serde_json::from_slice(&bytes).unwrap()
}

async fn traverse(db: &Database, direction: OrderDirection, expected: [&str; 8]) {
    let mut after = None;
    for (index, id) in expected.into_iter().enumerate() {
        let page = page(db, direction, after).await.unwrap();
        let [edge] = page.edges.as_slice() else {
            panic!("expected one edge")
        };
        assert_eq!(edge.node.id, id);
        assert_eq!(page.page_info.has_next_page, index + 1 < expected.len());
        let envelope = cursor_envelope(&edge.cursor);
        let values =
            decode_keyset_cursor(&edge.cursor, envelope["order"].as_str().unwrap(), 2).unwrap();
        let (_, expected_amount) = ROWS.iter().find(|(row_id, _)| *row_id == id).unwrap();
        assert_eq!(
            values,
            [
                expected_amount.map_or(KeysetValue::Null, |amount| KeysetValue::String(
                    amount.into()
                )),
                KeysetValue::String(id.into()),
            ]
        );
        after = page.page_info.end_cursor;
    }

    // Backward traversal must reverse NULL placement in the database query.
    let mut before = None;
    for id in expected.into_iter().rev() {
        let page = KeysetAmount::keyset_connection_page_ordered(
            db,
            Default::default(),
            order(direction),
            KeysetConnectionInput {
                before,
                last: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let [edge] = page.edges.as_slice() else {
            panic!("expected one edge")
        };
        assert_eq!(edge.node.id, id);
        before = page.page_info.start_cursor;
    }
}

async fn assert_cursor_invalid(db: &Database, cursor: String) {
    let error = page(db, OrderDirection::Asc, Some(cursor))
        .await
        .unwrap_err();
    assert_eq!(
        error
            .extensions
            .and_then(|extensions| extensions.get("code").cloned()),
        Some(async_graphql::Value::from("CURSOR_INVALID"))
    );
}

async fn assert_invalid_decimal_cursors(db: &Database) {
    let first = page(db, OrderDirection::Asc, None).await.unwrap();
    let mut envelope = cursor_envelope(&first.edges[0].cursor);
    // A changed but valid value with the original checksum is rejected.
    envelope["values"][0]["v"] = json!("-9999999999999999.98");
    let stale: String = serde_json::to_vec(&envelope)
        .unwrap()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_cursor_invalid(db, format!("gomk1.{stale}")).await;

    // A valid checksum still cannot make an invalid Decimal operand usable.
    for value in [
        KeysetValue::Int(0),
        KeysetValue::String("12.345".into()),
        KeysetValue::String("12.3".into()),
        KeysetValue::String("10000000000000000.00".into()),
        KeysetValue::String("01.00".into()),
    ] {
        let cursor = encode_keyset_cursor(
            envelope["order"].as_str().unwrap(),
            vec![value, KeysetValue::String("c".into())],
        );
        assert_cursor_invalid(db, cursor).await;
    }
}

async fn assert_filter_parity(db: &Database) {
    use OrderDirection::{Asc, Desc};
    let pivot = "9999999999999999.98".parse::<Decimal>().unwrap();
    for (direction, filter, expected) in [
        (
            Asc,
            DecimalFilter {
                gt: Some(pivot),
                ..Default::default()
            },
            vec!["g"],
        ),
        (
            Desc,
            DecimalFilter {
                lt: Some(pivot),
                ..Default::default()
            },
            vec!["b", "f", "c"],
        ),
    ] {
        // Both pivot rows sort by the primary key, so the continuation after
        // the second one is a strict comparison plus the two trailing NULLs.
        let first = KeysetAmount::keyset_page_ordered(
            db,
            Default::default(),
            order(direction),
            KeysetPageInput {
                limit: Some(if direction == Asc { 5 } else { 3 }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(first.edges.last().unwrap().node.id, "d");
        let resumed = KeysetAmount::keyset_page_ordered(
            db,
            Default::default(),
            order(direction),
            KeysetPageInput {
                after: first.page_info.end_cursor,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let filtered = KeysetAmount::keyset_page_ordered(
            db,
            KeysetAmountWhereInput {
                amount: Some(filter),
                ..Default::default()
            },
            order(direction),
            Default::default(),
        )
        .await
        .unwrap();
        let filtered_ids: Vec<_> = filtered
            .edges
            .iter()
            .map(|edge| edge.node.id.as_str())
            .collect();
        assert_eq!(filtered_ids, expected);
        assert_eq!(
            resumed
                .edges
                .iter()
                .take_while(|edge| edge.node.amount.is_some())
                .map(|edge| edge.node.id.as_str())
                .collect::<Vec<_>>(),
            filtered_ids
        );
    }
}

#[tokio::test]
#[cfg_attr(
    not(feature = "sqlite"),
    ignore = "requires a disposable database container"
)]
async fn decimal_keyset_order_and_filter_compare_exact_stored_values() {
    #[cfg(feature = "sqlite")]
    let db = {
        let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "keyset-decimals",
                "owned test",
                &[KeysetAmount::metadata()],
            )
            .await
            .unwrap();
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .unwrap();
        db
    };
    #[cfg(feature = "postgres")]
    let (db, mut owner) = {
        let owner = owned_postgres::OwnedPostgres::start("keyset-decimals").unwrap();
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await
            .unwrap();
        (Database::new(pool), owner)
    };
    #[cfg(feature = "mssql")]
    let (db, mut owner) = {
        let owner = owned_mssql::OwnedSqlServer::start().await.unwrap();
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await
        .unwrap();
        (
            Database::builder(pool)
                .schema_policy(SchemaPolicy::ExternalWritable)
                .build(),
            owner,
        )
    };

    #[cfg(not(feature = "sqlite"))]
    <DefaultBackend as WriteBackend>::execute_with_binds(
        db.pool(),
        "CREATE TABLE keyset_amounts (id VARCHAR(40) PRIMARY KEY, stored_amount DECIMAL(18,2))",
        &[],
    )
    .await
    .unwrap();

    let dialect = <DefaultBackend as OrmBackend>::DIALECT;
    let insert = format!(
        "INSERT INTO keyset_amounts (id, stored_amount) VALUES ({}, {})",
        dialect.placeholder(1),
        dialect.placeholder(2)
    );
    let definition = DecimalDef::new(18, 2).unwrap();
    for (id, amount) in ROWS {
        let value = amount.map_or(SqlValue::DecimalNull(definition), |amount| {
            SqlValue::Decimal(DecimalValue::new(amount.parse().unwrap(), definition).unwrap())
        });
        <DefaultBackend as WriteBackend>::execute_with_binds(
            db.pool(),
            &insert,
            &[SqlValue::String(id.into()), value],
        )
        .await
        .unwrap();
    }

    assert_schema_and_catalog(&db);
    let response = schema_builder(db.clone()).finish()
        .execute("{ keysetAmountsKeyset(orderBy: [{amount: ASC}], page: {limit: 1}) { edges { node { id } } } }")
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["keysetAmountsKeyset"]["edges"],
        json!([{"node": {"id": "c"}}])
    );

    traverse(
        &db,
        OrderDirection::Asc,
        ["c", "f", "b", "a", "d", "g", "h", "z"],
    )
    .await;
    traverse(
        &db,
        OrderDirection::Desc,
        ["g", "a", "d", "b", "f", "c", "h", "z"],
    )
    .await;
    assert_invalid_decimal_cursors(&db).await;
    assert_filter_parity(&db).await;

    #[cfg(not(feature = "sqlite"))]
    owner.cleanup().unwrap();
}
