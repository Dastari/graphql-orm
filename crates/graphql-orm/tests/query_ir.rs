use graphql_orm::graphql::orm::{
    DatabaseBackend, DeleteQuery, FilterExpression, PaginationRequest, RenderedQuery, SelectQuery,
    SortExpression, SqlValue, render_delete_query, render_select_query,
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
