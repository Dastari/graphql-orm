//! Exact static signed64 generation, wire coercion, SQL bindings and matching.
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
use graphql_orm::async_graphql::{InputType, Schema, Value};
use graphql_orm::graphql::filters::{Int64Filter, IntFilter};
use graphql_orm::graphql::orm::*;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        table = "signed64_rows",
        plural = "Signed64Rows",
        backend = "sqlite",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        table = "signed64_rows",
        plural = "Signed64Rows",
        backend = "postgres",
        auth = "none"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        table = "signed64_rows",
        plural = "Signed64Rows",
        backend = "mssql",
        auth = "none"
    )
)]
struct Signed64Row {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "int64")]
    value: Option<i64>,
    #[filterable(type = "number")]
    narrow: i32,
}
schema_roots! { entities: [Signed64Row] }

const VALUES: &[i64] = &[
    i64::MIN,
    i32::MIN as i64 - 1,
    -1,
    0,
    i32::MAX as i64 + 1,
    2_200_000_000,
    1_800_000_000_000,
    9_007_199_254_740_993,
    i64::MAX,
];

#[test]
fn exact_wire_scalar_and_existing_i32_contract() {
    let eq_name = Int64Filter::graphql_semantic_filter_input()
        .operators
        .into_iter()
        .find(|op| op.operator == GraphqlSemanticFilterOperator::Equal)
        .unwrap()
        .graphql_name;
    for value in VALUES {
        for wire in [
            Value::String(value.to_string()),
            Value::Number((*value).into()),
        ] {
            let filter = Int64Filter::parse(Some(Value::Object(
                [(graphql_orm::async_graphql::Name::new(&eq_name), wire)]
                    .into_iter()
                    .collect(),
            )))
            .unwrap();
            assert_eq!(filter.eq, Some(*value));
            assert_eq!(
                Int64Filter::parse(Some(filter.to_value())).unwrap().eq,
                Some(*value)
            );
        }
    }
    for wire in [
        Value::String("9223372036854775808".into()),
        Value::String("-9223372036854775809".into()),
        Value::String("01".into()),
        Value::String("1e3".into()),
        graphql_orm::async_graphql::value!(1.5),
    ] {
        assert!(
            Int64Filter::parse(Some(Value::Object(
                [(graphql_orm::async_graphql::Name::new(&eq_name), wire)]
                    .into_iter()
                    .collect()
            )))
            .is_err()
        );
    }
    let _: Option<i32> = IntFilter {
        eq: Some(i32::MAX),
        ..Default::default()
    }
    .eq;
    let invalid = Int64Filter {
        in_list: Some(vec![0; 257]),
        ..Default::default()
    };
    assert!(invalid.validate().is_err());
    assert!(Int64Filter::parse(Some(invalid.to_value())).is_err());
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn sql_and_memory_preserve_every_operand_and_boolean_composition() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE signed64_rows (id TEXT PRIMARY KEY, value INTEGER, narrow INTEGER NOT NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut rows = Vec::new();
    for value in VALUES.iter().copied().map(Some).chain([None]) {
        let row = Signed64Row {
            id: graphql_orm::uuid::Uuid::new_v4(),
            value,
            narrow: 0,
        };
        sqlx::query("INSERT INTO signed64_rows (id,value,narrow) VALUES (?,?,?)")
            .bind(row.id.to_string())
            .bind(row.value)
            .bind(0_i32)
            .execute(&pool)
            .await
            .unwrap();
        rows.push(row);
    }
    for value in VALUES {
        for filter in [
            Int64Filter {
                eq: Some(*value),
                ..Default::default()
            },
            Int64Filter {
                ne: Some(*value),
                ..Default::default()
            },
            Int64Filter {
                lt: Some(*value),
                ..Default::default()
            },
            Int64Filter {
                lte: Some(*value),
                ..Default::default()
            },
            Int64Filter {
                gt: Some(*value),
                ..Default::default()
            },
            Int64Filter {
                gte: Some(*value),
                ..Default::default()
            },
            Int64Filter {
                in_list: Some(vec![*value]),
                ..Default::default()
            },
            Int64Filter {
                not_in: Some(vec![*value]),
                ..Default::default()
            },
        ] {
            let input = Signed64RowWhereInput {
                value: Some(filter),
                ..Default::default()
            };
            let (conditions, binds) = input.to_sql_conditions();
            assert!(matches!(binds.as_slice(), [SqlValue::Int(actual)] if actual == value));
            let found: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT id FROM signed64_rows WHERE {}",
                conditions.join(" AND ")
            ))
            .bind(*value)
            .fetch_all(&pool)
            .await
            .unwrap();
            for row in &rows {
                assert_eq!(
                    input.matches_entity(row).unwrap(),
                    found.contains(&row.id.to_string())
                );
            }
        }
    }
    for filter in [
        Int64Filter {
            is_null: Some(true),
            ..Default::default()
        },
        Int64Filter {
            is_null: Some(false),
            ..Default::default()
        },
        Int64Filter {
            in_list: Some(vec![]),
            not_in: Some(vec![]),
            ..Default::default()
        },
    ] {
        let input = Signed64RowWhereInput {
            value: Some(filter),
            ..Default::default()
        };
        let (conditions, binds) = input.to_sql_conditions();
        assert!(binds.is_empty());
        let sql = if conditions.is_empty() {
            "1=1".into()
        } else {
            conditions.join(" AND ")
        };
        let found: Vec<String> =
            sqlx::query_scalar(&format!("SELECT id FROM signed64_rows WHERE {sql}"))
                .fetch_all(&pool)
                .await
                .unwrap();
        for row in &rows {
            assert_eq!(
                input.matches_entity(row).unwrap(),
                found.contains(&row.id.to_string())
            );
        }
    }
    let atom = || Signed64RowWhereInput {
        value: Some(Int64Filter {
            eq: Some(i64::MAX),
            ..Default::default()
        }),
        ..Default::default()
    };
    let composed = Signed64RowWhereInput {
        and: Some(vec![atom()]),
        or: Some(vec![atom()]),
        not: Some(Box::new(Signed64RowWhereInput {
            value: Some(Int64Filter {
                eq: Some(i64::MIN),
                ..Default::default()
            }),
            ..Default::default()
        })),
        ..Default::default()
    };
    assert!(!composed.matches_entity(rows.last().unwrap()).unwrap());
    assert!(composed.matches_entity(&rows[VALUES.len() - 1]).unwrap());
}

#[test]
fn generated_metadata_and_sdl_expose_true_width() {
    let schema = Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("scalar Signed64"));
    assert!(sdl.contains("input Int64Filter"));
    let metadata = Signed64Row::graphql_semantic_metadata().unwrap();
    let wide = metadata
        .fields
        .iter()
        .find(|f| {
            f.filter_input.as_ref().is_some_and(|i|
        matches!(&i.type_ref, GraphqlSemanticTypeRef::Named { name, .. } if name == "Int64Filter"))
        })
        .unwrap();
    let contract = wide.filter_input.as_ref().unwrap();
    contract.validate().unwrap();
    assert_eq!(contract.operators.len(), 9);
    assert!(format!("{:?}", contract).contains("Signed64"));
    assert!(!format!("{:?}", contract).contains("RuntimeInt64"));
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn nullable_atoms_keep_sql_unknown_under_boolean_composition() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE signed64_rows (id TEXT PRIMARY KEY, value INTEGER, narrow INTEGER NOT NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let rows = [None, Some(i64::MAX), Some(0)].map(|value| Signed64Row {
        id: graphql_orm::uuid::Uuid::new_v4(),
        value,
        narrow: 0,
    });
    for row in &rows {
        sqlx::query("INSERT INTO signed64_rows VALUES (?,?,0)")
            .bind(row.id.to_string())
            .bind(row.value)
            .execute(&pool)
            .await
            .unwrap();
    }
    let atom = |f| Signed64RowWhereInput {
        value: Some(f),
        ..Default::default()
    };
    let negated = |f| Signed64RowWhereInput {
        not: Some(Box::new(atom(f))),
        ..Default::default()
    };
    let mut cases = vec![
        negated(Int64Filter {
            eq: Some(i64::MAX),
            ..Default::default()
        }),
        negated(Int64Filter {
            in_list: Some(vec![i64::MAX]),
            ..Default::default()
        }),
        negated(Int64Filter {
            not_in: Some(vec![i64::MAX]),
            ..Default::default()
        }),
    ];
    for is_null in [true, false] {
        cases.push(atom(Int64Filter {
            eq: Some(i64::MAX),
            is_null: Some(is_null),
            ..Default::default()
        }));
        cases.push(negated(Int64Filter {
            eq: Some(i64::MAX),
            is_null: Some(is_null),
            ..Default::default()
        }));
    }
    cases.push(Signed64RowWhereInput {
        or: Some(vec![
            negated(Int64Filter {
                eq: Some(i64::MAX),
                ..Default::default()
            }),
            atom(Int64Filter {
                eq: Some(0),
                ..Default::default()
            }),
        ]),
        ..Default::default()
    });
    for filter in cases {
        let (conditions, values) = filter.to_sql_conditions();
        let sql = format!(
            "SELECT id FROM signed64_rows WHERE {}",
            conditions.join(" AND ")
        );
        let mut query = sqlx::query_scalar::<_, String>(&sql);
        for value in values {
            let SqlValue::Int(value) = value else {
                panic!("exact int bind");
            };
            query = query.bind(value);
        }
        let selected = query.fetch_all(&pool).await.unwrap();
        for row in &rows {
            assert_eq!(
                filter.matches_entity(row).unwrap(),
                selected.contains(&row.id.to_string())
            );
        }
    }
}

#[tokio::test]
async fn list_limits_apply_before_decode_at_actual_operator_names() {
    use graphql_orm::async_graphql::{
        EmptyMutation, EmptySubscription, Name, Object, Request, Variables,
    };
    struct Admission;
    #[Object]
    impl Admission {
        async fn admit(&self, filter: Int64Filter) -> bool {
            filter.validate().is_ok()
        }
    }
    let schema = Schema::build(Admission, EmptyMutation, EmptySubscription).finish();
    let contract = Int64Filter::graphql_semantic_filter_input();
    for kind in [
        GraphqlSemanticFilterOperator::In,
        GraphqlSemanticFilterOperator::NotIn,
    ] {
        let operator = &contract
            .operators
            .iter()
            .find(|op| op.operator == kind)
            .unwrap()
            .graphql_name;
        for size in [256, 257] {
            let raw = Value::Object(
                [(
                    Name::new(operator),
                    Value::List(vec![Value::String("9223372036854775807".into()); size]),
                )]
                .into_iter()
                .collect(),
            );
            assert_eq!(Int64Filter::parse(Some(raw.clone())).is_ok(), size == 256);
            let variable = schema
                .execute(
                    Request::new("query($f: Int64Filter!) { admit(filter: $f) }").variables(
                        Variables::from_value(Value::Object(
                            [(Name::new("f"), raw)].into_iter().collect(),
                        )),
                    ),
                )
                .await;
            assert_eq!(variable.errors.is_empty(), size == 256);
            let literals = vec!["\"9223372036854775807\""; size].join(",");
            let literal = schema
                .execute(format!(
                    "{{ admit(filter: {{ {operator}: [{literals}] }}) }}"
                ))
                .await;
            assert_eq!(literal.errors.is_empty(), size == 256);
            let native = if kind == GraphqlSemanticFilterOperator::In {
                Int64Filter {
                    in_list: Some(vec![i64::MAX; size]),
                    ..Default::default()
                }
            } else {
                Int64Filter {
                    not_in: Some(vec![i64::MAX; size]),
                    ..Default::default()
                }
            };
            let nested = Signed64RowWhereInput {
                not: Some(Box::new(Signed64RowWhereInput {
                    or: Some(vec![Signed64RowWhereInput {
                        value: Some(native),
                        ..Default::default()
                    }]),
                    ..Default::default()
                })),
                ..Default::default()
            };
            assert_eq!(DatabaseFilter::validate(&nested).is_ok(), size == 256);
        }
        let singleton = schema
            .execute(format!(
                "{{ admit(filter: {{ {operator}: \"9223372036854775807\" }}) }}"
            ))
            .await;
        assert!(singleton.errors.is_empty(), "{:?}", singleton.errors);
        let singleton_variable = schema
            .execute(
                Request::new("query($f: Int64Filter!) { admit(filter: $f) }").variables(
                    Variables::from_value(Value::Object(
                        [(
                            Name::new("f"),
                            Value::Object(
                                [(Name::new(operator), Value::String("1".into()))]
                                    .into_iter()
                                    .collect(),
                            ),
                        )]
                        .into_iter()
                        .collect(),
                    )),
                ),
            )
            .await;
        assert!(
            singleton_variable.errors.is_empty(),
            "{:?}",
            singleton_variable.errors
        );
        assert!(
            Int64Filter::parse(Some(Value::Object(
                [(Name::new(operator), Value::String("1".into()))]
                    .into_iter()
                    .collect()
            )))
            .is_ok()
        );
        // Oversize invalid elements must report the bound, not element decoding.
        let error = Int64Filter::parse(Some(Value::Object(
            [(
                Name::new(operator),
                Value::List(vec![Value::Boolean(true); 257]),
            )]
            .into_iter()
            .collect(),
        )))
        .unwrap_err();
        assert!(
            error
                .into_server_error(Default::default())
                .message
                .contains("exceeds 256")
        );
    }
}

#[cfg(feature = "sqlite")]
mod spatial_unknown {
    use super::*;
    use graphql_orm::graphql::filters::SpatialFilter;
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        table = "spatial64",
        plural = "Spatial64Rows",
        backend = "sqlite",
        auth = "none"
    )]
    struct Spatial64 {
        #[primary_key]
        #[sortable]
        id: graphql_orm::uuid::Uuid,
        #[filterable(type = "int64")]
        value: Option<i64>,
        #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326))]
        #[filterable(type = "spatial")]
        location: graphql_orm::serde_json::Value,
    }
    #[tokio::test]
    async fn nested_or_not_spatial_prefilter_retains_unknown() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE spatial64 (id TEXT PRIMARY KEY, value INTEGER, location TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut expected = Vec::new();
        for (value, x) in [(None, 1), (Some(i64::MAX), 0), (Some(0), 1)] {
            let id = graphql_orm::uuid::Uuid::new_v4();
            let location = graphql_orm::serde_json::json!({"type":"Point","coordinates":[x,x]});
            sqlx::query("INSERT INTO spatial64 VALUES (?,?,?)")
                .bind(id.to_string())
                .bind(value)
                .bind(location.to_string())
                .execute(&pool)
                .await
                .unwrap();
            if x == 0 || value.is_some_and(|v| v != i64::MAX) {
                expected.push(id);
            }
        }
        let filter = Spatial64WhereInput {
            or: Some(vec![
                Spatial64WhereInput {
                    not: Some(Box::new(Spatial64WhereInput {
                        value: Some(Int64Filter {
                            eq: Some(i64::MAX),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                Spatial64WhereInput {
                    location: Some(SpatialFilter {
                        intersects: Some(graphql_orm::async_graphql::Json(
                            graphql_orm::serde_json::json!({"type":"Point","coordinates":[0,0]}),
                        )),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let mut actual = Spatial64::query(&pool)
            .filter(filter)
            .fetch_all()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect::<Vec<_>>();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
    }
}
