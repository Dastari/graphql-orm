use graphql_orm::graphql::orm::{
    DatabaseBackend, DeleteQuery, FilterExpression, PaginationRequest, RenderedQuery, SelectQuery,
    SortExpression, SqlDialect, SqlValue, render_delete_query, render_select_query,
};

fn sample_select() -> SelectQuery {
    SelectQuery {
        table: "users",
        columns: vec!["id".to_string(), "name".to_string()],
        filter: Some(FilterExpression::And(vec![
            FilterExpression::Raw {
                clause: "active = ?".to_string(),
                values: vec![SqlValue::Bool(true)],
            },
            FilterExpression::Raw {
                clause: "name LIKE ?".to_string(),
                values: vec![SqlValue::String("%Al%".to_string())],
            },
        ])),
        sorts: vec![SortExpression {
            clause: "name ASC".to_string(),
        }],
        pagination: Some(PaginationRequest {
            limit: Some(10),
            offset: 20,
        }),
        count_only: false,
    }
}

#[test]
fn sqlite_renderer_keeps_qmark_placeholders() {
    let rendered: RenderedQuery = render_select_query(DatabaseBackend::Sqlite, &sample_select());
    assert!(rendered.sql.contains("active = ?"));
    assert!(rendered.sql.contains("name LIKE ?"));
    assert!(rendered.sql.contains("ORDER BY name ASC"));
    assert!(rendered.sql.contains("LIMIT 10 OFFSET 20"));
    assert_eq!(rendered.values.len(), 2);
}

#[test]
fn postgres_renderer_numbers_placeholders() {
    let rendered = render_select_query(DatabaseBackend::Postgres, &sample_select());
    assert!(rendered.sql.contains("active = $1"));
    assert!(rendered.sql.contains("name LIKE $2"));
    assert_eq!(rendered.values.len(), 2);
}

#[test]
fn mssql_dialect_quotes_identifiers_and_renders_helpers() {
    let dialect = DatabaseBackend::Mssql;

    assert_eq!(dialect.quote_identifier("Job Name"), "[Job Name]");
    assert_eq!(dialect.quote_identifier("Job]Name"), "[Job]]Name]");
    assert_eq!(dialect.quote_identifier_path("dbo.Jobs"), "[dbo].[Jobs]");
    assert_eq!(dialect.placeholder(3), "@P3");
    assert_eq!(
        dialect.ci_like("[JobName]", "@P1"),
        "LOWER([JobName]) LIKE LOWER(@P1)"
    );
    assert_eq!(
        dialect.relation_key_cast("[JobId]"),
        "CAST([JobId] AS NVARCHAR(4000))"
    );
    assert_eq!(
        dialect.days_ago_expr(7),
        "DATEADD(day, -7, CAST(GETDATE() AS date))"
    );
    assert_eq!(
        dialect.current_epoch_expr(),
        "DATEDIFF_BIG(second, '1970-01-01', SYSUTCDATETIME())"
    );
}

#[test]
fn mssql_renderer_numbers_placeholders_orders_and_paginates() {
    let rendered = render_select_query(
        DatabaseBackend::Mssql,
        &SelectQuery {
            table: "[dbo].[Jobs]",
            columns: vec!["[Id]".to_string(), "[JobName]".to_string()],
            filter: Some(FilterExpression::And(vec![
                FilterExpression::Raw {
                    clause: "[JobName] LIKE ?".to_string(),
                    values: vec![SqlValue::String("%welder%".to_string())],
                },
                FilterExpression::Raw {
                    clause: "[ClosedAt] IS NULL".to_string(),
                    values: vec![],
                },
                FilterExpression::Raw {
                    clause: "[CustomerId] = ?".to_string(),
                    values: vec![SqlValue::Int(42)],
                },
            ])),
            sorts: vec![SortExpression {
                clause: "[JobName] DESC".to_string(),
            }],
            pagination: Some(PaginationRequest {
                limit: Some(10),
                offset: 20,
            }),
            count_only: false,
        },
    );

    assert_eq!(
        rendered.sql,
        "SELECT [Id], [JobName] FROM [dbo].[Jobs] WHERE ([JobName] LIKE @P1) AND ([ClosedAt] IS NULL) AND ([CustomerId] = @P2) ORDER BY [JobName] DESC OFFSET 20 ROWS FETCH NEXT 10 ROWS ONLY"
    );
    assert_eq!(
        rendered.values,
        vec![SqlValue::String("%welder%".to_string()), SqlValue::Int(42)]
    );
}

#[test]
fn mssql_renderer_uses_count_big_and_default_pagination_order() {
    let count = render_select_query(
        DatabaseBackend::Mssql,
        &SelectQuery {
            table: "[dbo].[Jobs]",
            columns: vec!["[Id]".to_string()],
            filter: Some(FilterExpression::Raw {
                clause: "[StartedAt] >= @P1".to_string(),
                values: vec![SqlValue::String("2026-01-01".to_string())],
            }),
            sorts: vec![],
            pagination: Some(PaginationRequest {
                limit: Some(25),
                offset: 5,
            }),
            count_only: true,
        },
    );
    assert_eq!(
        count.sql,
        "SELECT COUNT_BIG(*) AS [count] FROM [dbo].[Jobs] WHERE [StartedAt] >= @P1"
    );
    assert_eq!(count.values.len(), 1);

    let paginated = render_select_query(
        DatabaseBackend::Mssql,
        &SelectQuery {
            table: "[dbo].[Jobs]",
            columns: vec!["[Id]".to_string()],
            filter: None,
            sorts: vec![],
            pagination: Some(PaginationRequest {
                limit: Some(25),
                offset: 5,
            }),
            count_only: false,
        },
    );
    assert_eq!(
        paginated.sql,
        "SELECT [Id] FROM [dbo].[Jobs] ORDER BY (SELECT 1) OFFSET 5 ROWS FETCH NEXT 25 ROWS ONLY"
    );
}

#[test]
fn delete_renderer_uses_filter_ir() {
    let rendered = render_delete_query(
        DatabaseBackend::Postgres,
        &DeleteQuery {
            table: "users",
            filter: Some(FilterExpression::Raw {
                clause: "id = ?".to_string(),
                values: vec![SqlValue::String("u1".to_string())],
            }),
        },
    );

    assert_eq!(rendered.sql, "DELETE FROM users WHERE id = $1");
    assert_eq!(rendered.values, vec![SqlValue::String("u1".to_string())]);
}
