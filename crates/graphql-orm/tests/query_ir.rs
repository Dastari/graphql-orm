use graphql_orm::graphql::orm::{
    AggregateFunction, AggregateQuery, DatabaseBackend, DeleteQuery, FilterExpression, PageInput,
    PaginationConfig, PaginationRequest, RenderedQuery, SchemaLimits, SelectQuery, SortExpression,
    SpatialPredicate, SqlDialect, SqlValue, contains_like_pattern, render_aggregate_query,
    render_delete_query, render_select_query, search_json_path_text, validate_search_json_path,
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
fn placeholder_normalization_skips_quoted_literals() {
    let rendered = DatabaseBackend::Postgres.normalize_sql("note = '$1?' AND id = ?", 1);
    assert_eq!(rendered, "note = '$1?' AND id = $1");

    let rendered = DatabaseBackend::Mssql.normalize_sql("[note?] = '@P1' AND [id] = ?", 1);
    assert_eq!(rendered, "[note?] = '@P1' AND [id] = @P1");
}

#[test]
fn pagination_config_resolves_defaults_and_caps() {
    let config = PaginationConfig::default();
    assert_eq!(
        config.resolve_page(None, true),
        PaginationRequest {
            limit: Some(50),
            offset: 0,
        }
    );
    assert_eq!(
        config.resolve_page(
            Some(&PageInput {
                limit: Some(5001),
                offset: Some(-10),
            }),
            true,
        ),
        PaginationRequest {
            limit: Some(100),
            offset: 0,
        }
    );
    assert_eq!(
        PaginationConfig::unbounded().resolve_page(None, true),
        PaginationRequest {
            limit: None,
            offset: 0,
        }
    );
    let raised_cap = PaginationConfig::default().with_max_limit(Some(5_000));
    let large_page = PageInput {
        limit: Some(4_000),
        offset: Some(12),
    };
    assert_eq!(
        raised_cap.resolve_page(Some(&large_page), true),
        PaginationRequest {
            limit: Some(4_000),
            offset: 12,
        }
    );
    assert_eq!(large_page.limit_with_config(raised_cap), Some(4_000));
    #[allow(deprecated)]
    {
        assert_eq!(large_page.limit(), Some(100));
    }
    let legacy = PaginationConfig::legacy();
    assert_eq!(
        legacy.resolve_page(None, true),
        PaginationRequest {
            limit: Some(1000),
            offset: 0,
        }
    );
    assert_eq!(
        DatabaseBackend::Sqlite.render_pagination(Some(-5), -10),
        " LIMIT 0"
    );
    assert_eq!(
        DatabaseBackend::Postgres.render_pagination(Some(5001), 2),
        " LIMIT 5001 OFFSET 2"
    );
    assert_eq!(
        DatabaseBackend::Mssql.render_pagination(Some(5001), 2),
        " OFFSET 2 ROWS FETCH NEXT 5001 ROWS ONLY"
    );
}

#[test]
fn schema_limits_have_production_defaults_and_can_be_disabled() {
    assert_eq!(
        SchemaLimits::default(),
        SchemaLimits {
            max_depth: Some(16),
            max_complexity: Some(20_000),
        }
    );
    assert_eq!(
        SchemaLimits::unbounded(),
        SchemaLimits {
            max_depth: None,
            max_complexity: None,
        }
    );
    assert_eq!(
        SchemaLimits::default()
            .with_max_depth(Some(8))
            .with_max_complexity(None),
        SchemaLimits {
            max_depth: Some(8),
            max_complexity: None,
        }
    );
}

#[test]
fn like_patterns_escape_wildcards() {
    assert_eq!(contains_like_pattern(r"50%_off\sale"), r"%50\%\_off\\sale%");
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
        "LOWER([JobName]) LIKE LOWER(@P1) ESCAPE '\\'"
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

#[test]
fn search_json_path_extracts_supported_text_shapes() {
    let value = serde_json::json!({
        "description": { "summary": "Public description" },
        "historical": { "summary": null },
        "classification": {
            "primary": { "label": "Architecture" },
            "keywords": [
                { "label": "heritage" },
                { "label": "civic" },
                { "label": 42 },
                {}
            ]
        },
        "tags": [
            { "value": "blue" },
            { "value": "stone" }
        ],
        "score": 12
    });

    assert_eq!(
        search_json_path_text(&value, "$.description.summary"),
        "Public description"
    );
    assert_eq!(
        search_json_path_text(&value, "$.classification.primary.label"),
        "Architecture"
    );
    assert_eq!(
        search_json_path_text(&value, "$.classification.keywords[*].label"),
        "heritage civic"
    );
    assert_eq!(
        search_json_path_text(&value["tags"], "$[*].value"),
        "blue stone"
    );
    assert_eq!(search_json_path_text(&value, "$.missing.summary"), "");
    assert_eq!(search_json_path_text(&value, "$.historical.summary"), "");
    assert_eq!(search_json_path_text(&value, "$.score"), "");
    assert_eq!(search_json_path_text(&value, "$.tags[*].missing"), "");
}

#[test]
fn search_json_path_validation_rejects_unsupported_syntax() {
    assert!(validate_search_json_path("$.description.summary").is_ok());
    assert!(validate_search_json_path("$.classification.keywords[*].label").is_ok());
    assert!(validate_search_json_path("$[*].value").is_ok());

    assert!(validate_search_json_path("description.summary").is_err());
    assert!(validate_search_json_path("$").is_err());
    assert!(validate_search_json_path("$..summary").is_err());
    assert!(validate_search_json_path("$.keywords[0].label").is_err());
    assert!(validate_search_json_path("$.keywords[*").is_err());
}

#[test]
fn aggregate_renderer_uses_filter_ir() {
    let rendered = render_aggregate_query(
        DatabaseBackend::Postgres,
        &AggregateQuery {
            table: "accession_assets",
            function: AggregateFunction::Max,
            column: Some("\"sort_order\"".to_string()),
            filter: Some(FilterExpression::Raw {
                clause: "accession_id = ?".to_string(),
                values: vec![SqlValue::String("a1".to_string())],
            }),
        },
    );

    assert_eq!(
        rendered.sql,
        "SELECT MAX(\"sort_order\") AS __gom_aggregate FROM accession_assets WHERE accession_id = $1"
    );
    assert_eq!(rendered.values, vec![SqlValue::String("a1".to_string())]);
}

#[test]
fn postgres_spatial_dialect_renders_geojson_predicates() {
    let dialect = DatabaseBackend::Postgres;
    let geometry = dialect.spatial_geojson_expr("$1", 4326);
    assert_eq!(geometry, "ST_SetSRID(ST_GeomFromGeoJSON($1::jsonb), 4326)");

    let cases = [
        (
            SpatialPredicate::Equals,
            "ST_Equals(location, ST_SetSRID(ST_GeomFromGeoJSON($1::jsonb), 4326))",
        ),
        (
            SpatialPredicate::Disjoint,
            "NOT ST_Intersects(location, ST_SetSRID(ST_GeomFromGeoJSON($1::jsonb), 4326))",
        ),
        (
            SpatialPredicate::Intersects,
            "ST_Intersects(location, ST_SetSRID(ST_GeomFromGeoJSON($1::jsonb), 4326))",
        ),
        (
            SpatialPredicate::Touches,
            "ST_Touches(location, ST_SetSRID(ST_GeomFromGeoJSON($1::jsonb), 4326))",
        ),
        (
            SpatialPredicate::Crosses,
            "ST_Crosses(location, ST_SetSRID(ST_GeomFromGeoJSON($1::jsonb), 4326))",
        ),
        (
            SpatialPredicate::Within,
            "ST_Within(location, ST_SetSRID(ST_GeomFromGeoJSON($1::jsonb), 4326))",
        ),
        (
            SpatialPredicate::Contains,
            "ST_Contains(location, ST_SetSRID(ST_GeomFromGeoJSON($1::jsonb), 4326))",
        ),
        (
            SpatialPredicate::Overlaps,
            "ST_Overlaps(location, ST_SetSRID(ST_GeomFromGeoJSON($1::jsonb), 4326))",
        ),
    ];

    for (predicate, expected) in cases {
        assert_eq!(
            dialect.spatial_predicate(predicate, "location", &geometry),
            expected
        );
    }
}
