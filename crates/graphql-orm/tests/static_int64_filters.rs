//! Exact static signed64 generation, wire coercion, SQL bindings and matching.
#![cfg(feature = "sqlite")]
use graphql_orm::async_graphql::{InputType, Schema, Value};
use graphql_orm::graphql::filters::{Int64Filter, IntFilter};
use graphql_orm::graphql::orm::*;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "signed64_rows", plural = "Signed64Rows", backend = "sqlite", auth = "none")]
struct Signed64Row {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "int64")]
    value: Option<i64>,
    #[filterable(type = "number")]
    narrow: i32,
}
schema_roots! { entities: [Signed64Row] }

const VALUES: &[i64] = &[i64::MIN, i32::MIN as i64 - 1, -1, 0,
    i32::MAX as i64 + 1, 2_200_000_000, 1_800_000_000_000,
    9_007_199_254_740_993, i64::MAX];

#[test]
fn exact_wire_scalar_and_existing_i32_contract() {
    for value in VALUES {
        for wire in [Value::String(value.to_string()), Value::Number((*value).into())] {
            let filter = Int64Filter::parse(Some(Value::Object(
                [("eq".into(), wire)].into_iter().collect()))).unwrap();
            assert_eq!(filter.eq, Some(*value));
            assert_eq!(Int64Filter::parse(Some(filter.to_value())).unwrap().eq, Some(*value));
        }
    }
    for wire in [Value::String("9223372036854775808".into()),
        Value::String("-9223372036854775809".into()), Value::String("01".into()),
        Value::String("1e3".into()), Value::Number(1.5_f64.try_into().unwrap())] {
        assert!(Int64Filter::parse(Some(Value::Object(
            [("eq".into(), wire)].into_iter().collect()))).is_err());
    }
    let _: Option<i32> = IntFilter { eq: Some(i32::MAX), ..Default::default() }.eq;
    let invalid = Int64Filter { in_list: Some(vec![0; 257]), ..Default::default() };
    assert!(invalid.validate().is_err());
    assert!(Int64Filter::parse(Some(invalid.to_value())).is_err());
}

#[tokio::test]
async fn sql_and_memory_preserve_every_operand_and_boolean_composition() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
        .connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE signed64_rows (id TEXT PRIMARY KEY, value INTEGER, narrow INTEGER NOT NULL)")
        .execute(&pool).await.unwrap();
    let mut rows = Vec::new();
    for value in VALUES.iter().copied().map(Some).chain([None]) {
        let row = Signed64Row { id: graphql_orm::uuid::Uuid::new_v4(), value, narrow: 0 };
        sqlx::query("INSERT INTO signed64_rows (id,value,narrow) VALUES (?,?,?)")
            .bind(row.id.to_string()).bind(row.value).bind(0_i32).execute(&pool).await.unwrap();
        rows.push(row);
    }
    for value in VALUES {
        for filter in [
            Int64Filter { eq: Some(*value), ..Default::default() },
            Int64Filter { ne: Some(*value), ..Default::default() },
            Int64Filter { lt: Some(*value), ..Default::default() },
            Int64Filter { lte: Some(*value), ..Default::default() },
            Int64Filter { gt: Some(*value), ..Default::default() },
            Int64Filter { gte: Some(*value), ..Default::default() },
            Int64Filter { in_list: Some(vec![*value]), ..Default::default() },
            Int64Filter { not_in: Some(vec![*value]), ..Default::default() },
        ] {
            let input = Signed64RowWhereInput { value: Some(filter), ..Default::default() };
            let (conditions, binds) = input.to_sql_conditions();
            assert!(matches!(binds.as_slice(), [SqlValue::Int(actual)] if actual == value));
            let found: Vec<String> = sqlx::query_scalar(&format!("SELECT id FROM signed64_rows WHERE {}", conditions.join(" AND ")))
                .bind(*value).fetch_all(&pool).await.unwrap();
            for row in &rows { assert_eq!(input.matches_entity(row).unwrap(), found.contains(&row.id.to_string())); }
        }
    }
    for filter in [Int64Filter { is_null: Some(true), ..Default::default() },
        Int64Filter { is_null: Some(false), ..Default::default() },
        Int64Filter { in_list: Some(vec![]), not_in: Some(vec![]), ..Default::default() }] {
        let input = Signed64RowWhereInput { value: Some(filter), ..Default::default() };
        let (conditions, binds) = input.to_sql_conditions();
        assert!(binds.is_empty());
        let sql = if conditions.is_empty() { "1=1".into() } else { conditions.join(" AND ") };
        let found: Vec<String> = sqlx::query_scalar(&format!("SELECT id FROM signed64_rows WHERE {sql}"))
            .fetch_all(&pool).await.unwrap();
        for row in &rows { assert_eq!(input.matches_entity(row).unwrap(), found.contains(&row.id.to_string())); }
    }
    let atom = || Signed64RowWhereInput { value: Some(Int64Filter { eq: Some(i64::MAX), ..Default::default() }), ..Default::default() };
    let composed = Signed64RowWhereInput { and: Some(vec![atom()]), or: Some(vec![atom()]),
        not: Some(Box::new(Signed64RowWhereInput { value: Some(Int64Filter { eq: Some(i64::MIN), ..Default::default() }), ..Default::default() })), ..Default::default() };
    assert!(composed.matches_entity(rows.last().unwrap()).unwrap() == false);
    assert!(composed.matches_entity(&rows[VALUES.len()-1]).unwrap());
}

#[test]
fn generated_metadata_and_sdl_expose_true_width() {
    let schema = Schema::build(QueryRoot::default(), MutationRoot::default(), SubscriptionRoot::default()).finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("scalar Signed64"));
    assert!(sdl.contains("input Int64Filter"));
    let metadata = Signed64Row::graphql_semantic_metadata().unwrap();
    let wide = metadata.fields.iter().find(|f| f.filter_input.as_ref().is_some_and(|i|
        matches!(&i.type_ref, GraphqlSemanticTypeRef::Named { name, .. } if name == "Int64Filter"))).unwrap();
    let contract = wide.filter_input.as_ref().unwrap();
    contract.validate().unwrap();
    assert_eq!(contract.operators.len(), 9);
    assert!(format!("{:?}", contract).contains("Signed64"));
    assert!(!format!("{:?}", contract).contains("RuntimeInt64"));
}
