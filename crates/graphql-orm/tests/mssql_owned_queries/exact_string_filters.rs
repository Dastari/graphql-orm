use super::owned_mssql::OwnedSqlServer;
use graphql_orm::db::mssql::{MssqlPool, MssqlPoolOptions};
use graphql_orm::graphql::orm::{SqlValue, WriteBackend};
use graphql_orm::prelude::*;
use std::time::Duration;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.ExactFilterRows",
    exact_string_filters = true,
    index(name = "IX_ExactFilter_Code", columns = ["code"])
)]
struct ExactFilterRow {
    #[primary_key]
    #[filterable(type = "number")]
    id: i32,
    #[filterable(type = "string")]
    code: Option<String>,
    #[filterable(type = "string")]
    unindexed: Option<String>,
}

fn eq(value: &str) -> StringFilter {
    StringFilter {
        eq: Some(value.into()),
        ..Default::default()
    }
}

fn in_list(values: &[&str]) -> StringFilter {
    StringFilter {
        in_list: Some(values.iter().map(|value| (*value).into()).collect()),
        ..Default::default()
    }
}

#[test]
fn exact_filter_adds_seek_only_for_declared_bin2_columns() {
    let filter = ExactFilterRowWhereInput {
        code: Some(eq("ready")),
        unindexed: Some(in_list(&["ready", "running"])),
        ..Default::default()
    };
    let (conditions, values) = filter.to_sql_conditions();
    assert_eq!(
        conditions,
        [
            "((CASE WHEN [code] IS NOT NULL THEN CONVERT(nvarchar(max), [code]) + N'.' END) COLLATE Latin1_General_100_BIN2 = CONVERT(nvarchar(max), @P1) + N'.')",
            "[code] = @P2",
            "(([unindexed] = @P3 COLLATE Latin1_General_100_BIN2 AND DATALENGTH(CONVERT(nvarchar(max), [unindexed])) = DATALENGTH(@P4)) OR ([unindexed] = @P5 COLLATE Latin1_General_100_BIN2 AND DATALENGTH(CONVERT(nvarchar(max), [unindexed])) = DATALENGTH(@P6)))",
        ]
    );
    let different_lengths = ExactFilterRowWhereInput {
        code: Some(eq("🚀")),
        unindexed: Some(in_list(&["", "x "])),
        ..Default::default()
    };
    assert_eq!(different_lengths.to_sql_conditions().0, conditions);
    assert_eq!(
        values,
        ["ready", "ready", "ready", "ready", "running", "running"]
            .map(|value| SqlValue::String(value.into()))
    );
}

#[test]
fn exact_membership_keeps_the_bind_budget() {
    use graphql_orm::graphql::filters::MAX_FILTER_LIST_OPERANDS;

    let filter = ExactFilterRowWhereInput {
        code: Some(in_list(&vec!["ready"; MAX_FILTER_LIST_OPERANDS])),
        ..Default::default()
    };
    let (conditions, values) = filter.to_sql_conditions();
    let different_lengths = ExactFilterRowWhereInput {
        code: Some(in_list(&vec!["🚀 "; MAX_FILTER_LIST_OPERANDS])),
        ..Default::default()
    };
    assert_eq!(different_lengths.to_sql_conditions().0, conditions);
    assert_eq!(values.len(), MAX_FILTER_LIST_OPERANDS * 2);
    assert!(conditions[1].starts_with("[code] IN ("));
}

#[tokio::test]
#[ignore = "requires test-owned SQL Server container"]
async fn exact_filters_seek_and_preserve_rows_and_read_visibility() {
    let mut server = OwnedSqlServer::start().await.unwrap();
    let pool = MssqlPool::connect_ado_external_writable_with(
        &server.connection_string,
        MssqlPoolOptions::new().query_timeout(Some(Duration::from_secs(15))),
    )
    .await
    .unwrap();
    let db = Database::<MssqlBackend>::new(pool);
    <MssqlBackend as WriteBackend>::execute_with_binds(
        db.pool(),
        "CREATE TABLE dbo.ExactFilterRows (
             id int NOT NULL PRIMARY KEY,
             code nvarchar(450) COLLATE Latin1_General_100_BIN2 NULL,
             unindexed nvarchar(max) COLLATE Latin1_General_100_CI_AI NULL
         );
         CREATE INDEX IX_ExactFilter_Code ON dbo.ExactFilterRows(code);
         INSERT dbo.ExactFilterRows (id, code, unindexed)
         SELECT id, code, code FROM (VALUES
             (1, N'ready'), (2, N'ready '), (3, N'READY'), (4, NULL),
             (5, N'🚀'), (6, N'réady'), (7, N'🚀 '), (8, N''),
             (9, N' '), (10, N'a' + NCHAR(0)), (11, N'a'), (12, N'𐐷'), (13, NULL)
         ) AS v(id, code);
         WITH digits(n) AS (SELECT n FROM (VALUES (0),(1),(2),(3),(4),(5),(6),(7)) AS d(n))
         INSERT dbo.ExactFilterRows (id, code, unindexed)
         SELECT 100 + ROW_NUMBER() OVER (ORDER BY a.n,b.n,c.n,d.n), N'other', N'other'
         FROM digits a CROSS JOIN digits b CROSS JOIN digits c CROSS JOIN digits d;
         UPDATE STATISTICS dbo.ExactFilterRows WITH FULLSCAN;",
        &[],
    )
    .await
    .unwrap();

    let ne = |value: &str| StringFilter {
        ne: Some(value.into()),
        ..Default::default()
    };
    let not_in = |values: &[&str]| StringFilter {
        not_in: in_list(values).in_list,
        ..Default::default()
    };

    for (name, filter, expected) in [
        ("eq", eq("ready"), vec![1]),
        ("eq_empty", eq(""), vec![8]),
        ("eq_space", eq(" "), vec![9]),
        ("eq_nul", eq("a\0"), vec![10]),
        ("ne", ne("🚀"), vec![1, 2, 3, 6, 7, 8, 9, 10, 11, 12]),
        (
            "in_list",
            in_list(&["ready ", "🚀", "", "a\0", "𐐷"]),
            vec![2, 5, 8, 10, 12],
        ),
        (
            "not_in",
            not_in(&["ready ", "🚀", "", "a\0", "𐐷"]),
            vec![1, 3, 6, 7, 9, 11],
        ),
        (
            "combined",
            StringFilter {
                eq: Some("ready ".into()),
                ne: Some("READY".into()),
                in_list: in_list(&["ready", "ready ", "READY"]).in_list,
                not_in: not_in(&["ready", "🚀"]).not_in,
                ..Default::default()
            },
            vec![2],
        ),
        ("empty_list", in_list(&[]), vec![]),
    ] {
        let (legacy_sql, legacy_binds) = legacy_predicate(&filter);
        let new = ExactFilterRowWhereInput {
            code: Some(filter),
            ..Default::default()
        };
        let (conditions, values) = new.to_sql_conditions();
        let new_sql = conditions.join(" AND ");
        let old_rows =
            select_ids(&db, &format!("id < 100 AND ({legacy_sql})"), &legacy_binds).await;
        assert_eq!(old_rows, expected, "{name}: legacy");
        assert_eq!(filter_ids(&db, &new).await, old_rows, "{name}");
        if matches!(name, "eq" | "in_list") {
            for setting in ["ON", "OFF"] {
                let marker = format!("/* exact_filter_seek_{name}_{setting} */");
                let sql = format!(
                    "SET CONCAT_NULL_YIELDS_NULL {setting};
                     SELECT id FROM dbo.ExactFilterRows WHERE {marker} {new_sql} ORDER BY id;
                     SET CONCAT_NULL_YIELDS_NULL ON;"
                );
                assert_eq!(
                    fetch_ids(&db, &sql, &values).await,
                    old_rows,
                    "{name}: seek with CONCAT_NULL_YIELDS_NULL {setting}"
                );
                assert_code_seek(db.pool(), &marker).await;
            }
        }

        let unindexed = ExactFilterRowWhereInput {
            unindexed: new.code.clone(),
            ..Default::default()
        };
        assert_eq!(
            filter_ids(&db, &unindexed).await,
            old_rows,
            "{name}: non-seek"
        );

        let policy = ReadPredicate::from_filter::<MssqlBackend, _>(&new).unwrap();
        assert_eq!(policy_ids(&db, &policy).await, old_rows, "{name}: policy");
        if name == "empty_list" {
            assert!(policy.not().is_err());
            continue;
        }

        // Include one NULL row through an OR arm, then check that NOT of the
        // OR still excludes UNKNOWN when the other arm is false.
        let include = ExactFilterRowWhereInput {
            id: Some(IntFilter {
                in_list: Some(vec![4, 11]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let include_policy = ReadPredicate::from_filter::<MssqlBackend, _>(&include).unwrap();
        let or = ExactFilterRowWhereInput {
            or: Some(vec![new.clone(), include]),
            ..Default::default()
        };
        for (filter, policy, baseline) in [
            (
                ExactFilterRowWhereInput {
                    not: Some(Box::new(new)),
                    ..Default::default()
                },
                policy.clone().not().unwrap(),
                format!("NOT ({legacy_sql})"),
            ),
            (
                or.clone(),
                policy.clone().or(include_policy.clone()).unwrap(),
                format!("(({legacy_sql}) OR id IN (4, 11))"),
            ),
            (
                ExactFilterRowWhereInput {
                    not: Some(Box::new(or)),
                    ..Default::default()
                },
                policy.or(include_policy).unwrap().not().unwrap(),
                format!("NOT (({legacy_sql}) OR id IN (4, 11))"),
            ),
        ] {
            let old_rows =
                select_ids(&db, &format!("id < 100 AND ({baseline})"), &legacy_binds).await;
            assert_eq!(
                filter_ids(&db, &filter).await,
                old_rows,
                "{name}: {baseline}"
            );
            let filter_policy = ReadPredicate::from_filter::<MssqlBackend, _>(&filter).unwrap();
            assert_eq!(
                policy_ids(&db, &filter_policy).await,
                old_rows,
                "{name}: nested filter as policy {baseline}"
            );
            assert_eq!(
                policy_ids(&db, &policy).await,
                old_rows,
                "{name}: policy {baseline}"
            );
        }
    }
    assert_exact_filter_matrix(&db, false).await;
    server.cleanup().unwrap();
}

#[tokio::test]
#[ignore = "requires test-owned SQL Server container"]
async fn exact_filters_match_unicode_operands_against_live_varchar() {
    let mut server = OwnedSqlServer::start().await.unwrap();
    let pool = MssqlPool::connect_ado_external_writable_with(
        &server.connection_string,
        MssqlPoolOptions::new().query_timeout(Some(Duration::from_secs(15))),
    )
    .await
    .unwrap();
    let db = Database::<MssqlBackend>::new(pool);
    <MssqlBackend as WriteBackend>::execute_with_binds(
        db.pool(),
        "CREATE TABLE dbo.ExactFilterRows (
             id int NOT NULL PRIMARY KEY,
             code varchar(8000) COLLATE Latin1_General_100_BIN2_UTF8 NULL,
             unindexed varchar(8000) COLLATE Latin1_General_100_CI_AI_SC_UTF8 NULL
         );
         INSERT dbo.ExactFilterRows (id, code, unindexed)
         SELECT id, code, code FROM (VALUES
             (1, N'bob'), (2, N'bob   '), (3, N'BOB'), (4, NULL),
             (5, REPLICATE(CONVERT(nvarchar(max), N'x'), 3999) + N' '),
             (6, REPLICATE(CONVERT(nvarchar(max), N'x'), 3999) + N'  '),
             (7, N'🚀'), (8, N'🚀 '),
             (9, REPLICATE(CONVERT(nvarchar(max), N'x'), 4000) + N'a'),
             (10, REPLICATE(CONVERT(nvarchar(max), N'x'), 4000) + N'b')
         ) AS v(id, code);",
        &[],
    )
    .await
    .unwrap();

    let (legacy, binds) = legacy_predicate(&eq("bob"));
    assert_eq!(select_ids(&db, &legacy, &binds).await, [2]);

    let long = format!("{} ", "x".repeat(3999));
    let longer = format!("{}a", "x".repeat(4000));
    let cases = [
        (eq("bob"), vec![1]),
        (in_list(&["bob", "BOB"]), vec![1, 3]),
        (
            StringFilter {
                ne: Some("bob".into()),
                ..Default::default()
            },
            vec![2, 3, 5, 6, 7, 8, 9, 10],
        ),
        (
            StringFilter {
                not_in: in_list(&["bob", "BOB"]).in_list,
                ..Default::default()
            },
            vec![2, 5, 6, 7, 8, 9, 10],
        ),
        (eq(&long), vec![5]),
        (eq(&longer), vec![9]),
        (eq("🚀"), vec![7]),
    ];
    for (filter, expected) in cases {
        for input in [
            ExactFilterRowWhereInput {
                code: Some(filter.clone()),
                ..Default::default()
            },
            ExactFilterRowWhereInput {
                unindexed: Some(filter.clone()),
                ..Default::default()
            },
        ] {
            let (sql, binds) = input.to_sql_conditions();
            assert_eq!(select_ids(&db, &sql.join(" AND "), &binds).await, expected);
            let policy = ReadPredicate::from_filter::<MssqlBackend, _>(&input).unwrap();
            assert_eq!(policy_ids(&db, &policy).await, expected);
        }
    }
    assert_exact_filter_matrix(&db, true).await;
    server.cleanup().unwrap();
}

async fn assert_exact_filter_matrix(db: &Database<MssqlBackend>, live_varchar: bool) {
    let operands = [
        "",
        " ",
        "  ",
        "bob",
        "bob ",
        "bob   ",
        "BOB",
        "bób",
        "e\u{301}",
        "é",
        "中",
        "🚀",
        "🚀 ",
        "𐐷",
        "a",
        "a.",
        "a .",
        "a\0",
        "a\0 ",
        "\0",
        ".",
        "\t",
        "bob\u{a0}",
    ];
    <MssqlBackend as WriteBackend>::execute_with_binds(
        db.pool(),
        "DELETE FROM dbo.ExactFilterRows;
         INSERT dbo.ExactFilterRows (id, code, unindexed) VALUES (0, NULL, NULL);",
        &[],
    )
    .await
    .unwrap();
    for (offset, operand) in operands.iter().enumerate() {
        <MssqlBackend as WriteBackend>::execute_with_binds(
            db.pool(),
            "INSERT dbo.ExactFilterRows (id, code, unindexed) VALUES (@P1, @P2, @P2);",
            &[
                SqlValue::Int(offset as i64 + 1),
                SqlValue::String((*operand).into()),
            ],
        )
        .await
        .unwrap();
    }

    for setting in ["ON", "OFF"] {
        for operand in operands {
            for column in ["[code]", "[unindexed]"] {
                let mut filter = ExactFilterRowWhereInput::default();
                if column == "[code]" {
                    filter.code = Some(eq(operand));
                } else {
                    filter.unindexed = Some(eq(operand));
                }
                let (conditions, binds) = filter.to_sql_conditions();
                // The legacy comparison used byte length. Normalize its input
                // on drifted columns to compare against its Unicode semantics.
                let baseline_column = if live_varchar {
                    format!("CONVERT(nvarchar(max), {column})")
                } else {
                    column.to_owned()
                };
                let baseline = format!(
                    "{baseline_column} COLLATE Latin1_General_100_BIN2 = @P1 AND DATALENGTH({baseline_column}) = DATALENGTH(@P2)"
                );
                let generated = conditions.join(" AND ");
                for negation in ["", "NOT "] {
                    let differences = db
                        .pool()
                        .fetch_rows(
                            &format!(
                                "SET CONCAT_NULL_YIELDS_NULL {setting};
                                 SELECT id FROM dbo.ExactFilterRows
                                 WHERE CASE WHEN {negation}({baseline}) THEN 1 ELSE 0 END
                                    <> CASE WHEN {negation}({generated}) THEN 1 ELSE 0 END;
                                 SET CONCAT_NULL_YIELDS_NULL ON;"
                            ),
                            &binds,
                        )
                        .await
                        .unwrap();
                    assert!(
                        differences.is_empty(),
                        "varchar={live_varchar}, setting={setting}, {column}, {negation}operand={operand:?}"
                    );
                }
            }
        }
    }
}

fn fixture_scope() -> ExactFilterRowWhereInput {
    ExactFilterRowWhereInput {
        id: Some(IntFilter {
            lt: Some(100),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn filter_ids(db: &Database<MssqlBackend>, filter: &ExactFilterRowWhereInput) -> Vec<i32> {
    let mut ids = EntityQuery::<ExactFilterRow>::new()
        .filter(&fixture_scope())
        .filter(filter)
        .fetch_all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.id)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

async fn policy_ids(db: &Database<MssqlBackend>, policy: &ReadPredicate) -> Vec<i32> {
    let mut ids = EntityQuery::<ExactFilterRow>::new()
        .filter(&fixture_scope())
        .with_read_visibility(&ReadVisibility::Complete(policy.clone()))
        .unwrap()
        .fetch_all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.id)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

fn legacy_predicate(filter: &StringFilter) -> (String, Vec<SqlValue>) {
    let mut binds = Vec::new();
    let mut equal = |value: &String| {
        let index = binds.len() + 1;
        binds.push(SqlValue::String(value.clone()));
        binds.push(SqlValue::String(value.clone()));
        format!(
            "([code] COLLATE Latin1_General_100_BIN2 = @P{index} AND DATALENGTH([code]) = DATALENGTH(@P{}))",
            index + 1
        )
    };
    let mut clauses = Vec::new();
    if let Some(value) = &filter.eq {
        clauses.push(equal(value));
    }
    if let Some(value) = &filter.ne {
        clauses.push(format!("NOT {}", equal(value)));
    }
    for (list, negated) in [(&filter.in_list, false), (&filter.not_in, true)] {
        if let Some(list) = list {
            if list.is_empty() {
                if !negated {
                    clauses.push("1 = 0".into());
                }
            } else {
                let comparisons = list.iter().map(&mut equal).collect::<Vec<_>>().join(" OR ");
                let not = if negated { "NOT " } else { "" };
                clauses.push(format!("{not}({comparisons})"));
            }
        }
    }
    (clauses.join(" AND "), binds)
}

async fn select_ids(db: &Database<MssqlBackend>, predicate: &str, values: &[SqlValue]) -> Vec<i32> {
    fetch_ids(
        db,
        &format!("SELECT id FROM dbo.ExactFilterRows WHERE {predicate} ORDER BY id"),
        values,
    )
    .await
}

async fn fetch_ids(db: &Database<MssqlBackend>, sql: &str, values: &[SqlValue]) -> Vec<i32> {
    db.pool()
        .fetch_rows(sql, values)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
        .iter()
        .map(|row| row.try_get(0).unwrap())
        .collect()
}

async fn assert_code_seek(pool: &MssqlPool, marker: &str) {
    let plans = pool
        .fetch_rows(
            "SELECT p.query_plan FROM sys.dm_exec_query_stats q
             CROSS APPLY sys.dm_exec_sql_text(q.sql_handle) s
             CROSS APPLY sys.dm_exec_text_query_plan(q.plan_handle,
                 q.statement_start_offset, q.statement_end_offset) p
             WHERE s.text LIKE @P1 AND s.text NOT LIKE N'%dm_exec_query_stats%'",
            &[SqlValue::String(format!("%{marker}%"))],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| row.try_get::<String, _>(0).unwrap())
        .collect::<Vec<_>>();
    assert!(
        plans.iter().any(|plan| {
            plan.contains("PhysicalOp=\"Index Seek\"")
                && plan.contains("Index=\"[IX_ExactFilter_Code]\"")
                && plan.split("<SeekPredicates>").skip(1).any(|seek| {
                    seek.split_once("</SeekPredicates>")
                        .is_some_and(|(seek, _)| seek.contains("Column=\"code\""))
                })
        }),
        "no seek on code for {marker}: {plans:#?}"
    );
}
