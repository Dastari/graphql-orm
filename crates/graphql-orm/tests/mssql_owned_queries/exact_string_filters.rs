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
            "([code] COLLATE Latin1_General_100_BIN2 = @P1 AND DATALENGTH([code]) = 10)",
            "[code] = @P2",
            "(([unindexed] COLLATE Latin1_General_100_BIN2 = @P3 AND DATALENGTH([unindexed]) = DATALENGTH(@P4)) OR ([unindexed] COLLATE Latin1_General_100_BIN2 = @P5 AND DATALENGTH([unindexed]) = DATALENGTH(@P6)))",
        ]
    );
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
             (5, N'🚀'), (6, N'réady'), (7, N'🚀 '), (8, N'')
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

    for (name, filter, expected, rejected) in [
        ("eq", eq("ready"), vec![1], vec![2, 3, 5, 6, 7, 8]),
        (
            "in_list",
            in_list(&["ready ", "🚀", ""]),
            vec![2, 5, 8],
            vec![1, 3, 6, 7],
        ),
        ("empty", in_list(&[]), vec![], vec![]),
    ] {
        let old = ExactFilterRowWhereInput {
            unindexed: Some(filter.clone()),
            ..Default::default()
        };
        let new = ExactFilterRowWhereInput {
            code: Some(filter),
            ..Default::default()
        };
        let (conditions, values) = old.to_sql_conditions();
        let old_sql = conditions.join(" AND ").replace("[unindexed]", "[code]");
        let old_rows = select_ids(&db, &old_sql, &values).await;
        assert_eq!(old_rows, expected);

        let (conditions, values) = new.to_sql_conditions();
        let marker = format!("/* exact_filter_seek_{name} */");
        let new_sql = format!("{marker} {}", conditions.join(" AND "));
        assert_eq!(select_ids(&db, &new_sql, &values).await, old_rows);
        if name != "empty" {
            assert_code_seek(db.pool(), &marker).await;
        }

        for negate in [false, true] {
            let mut policy = ReadPredicate::from_filter::<MssqlBackend, _>(&new).unwrap();
            if negate {
                if name == "empty" {
                    // A known deny fallback cannot become a blanket grant.
                    assert!(policy.not().is_err());
                    continue;
                }
                policy = policy.not().unwrap();
            }
            let mut ids = EntityQuery::<ExactFilterRow>::new()
                .filter(&ExactFilterRowWhereInput {
                    id: Some(IntFilter {
                        lt: Some(100),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .with_read_visibility(&ReadVisibility::Complete(policy))
                .unwrap()
                .fetch_all(&db)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>();
            ids.sort_unstable();
            assert_eq!(ids, if negate { &rejected } else { &old_rows }.as_slice());
        }
    }
    server.cleanup().unwrap();
}

async fn select_ids(db: &Database<MssqlBackend>, predicate: &str, values: &[SqlValue]) -> Vec<i32> {
    db.pool()
        .fetch_rows(
            &format!("SELECT id FROM dbo.ExactFilterRows WHERE {predicate} ORDER BY id"),
            values,
        )
        .await
        .unwrap()
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
