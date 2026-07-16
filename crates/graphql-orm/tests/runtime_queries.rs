#![cfg(feature = "sqlite")]
//! Real SQLite coverage for validated runtime query execution.

use graphql_orm::graphql::orm::{
    CollectionId, FieldId, RuntimeCollection, RuntimeDateTime, RuntimeField, RuntimeFieldState,
    RuntimeFloat, RuntimeListOperator, RuntimeNullPlacement, RuntimeOrderDirection,
    RuntimeOrderInput, RuntimeOrderTerm, RuntimePageRequest, RuntimeQueryErrorCode,
    RuntimeQueryLimits, RuntimeScalarOperator, RuntimeSchema, RuntimeValue, RuntimeValueKind,
};

fn cid(value: &str) -> CollectionId {
    CollectionId::new(value).unwrap()
}

fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}

fn field(id: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        id: fid(id),
        api_name: id.to_string(),
        physical_column: id.to_string(),
        value_kind: kind,
        nullable,
        unique: id == "id",
        filterable: true,
        sortable: !matches!(kind, RuntimeValueKind::Json),
        generated: false,
        default: None,
    }
}

fn schema() -> graphql_orm::graphql::orm::ValidatedRuntimeSchema {
    let id = field("id", RuntimeValueKind::Integer, false);
    RuntimeSchema {
        format_version: 1,
        collections: vec![RuntimeCollection {
            id: cid("customers"),
            api_type_name: "Customer".to_string(),
            api_plural_name: "Customers".to_string(),
            physical_table: "runtime_query_customers".to_string(),
            primary_key: vec![id.id.clone()],
            append_only: false,
            retention_purge: false,
            fields: vec![
                id.clone(),
                field("status", RuntimeValueKind::String, false),
                field("rank", RuntimeValueKind::Integer, true),
                field("secret", RuntimeValueKind::Bytes, false),
                field("active", RuntimeValueKind::Boolean, false),
                field("score", RuntimeValueKind::Float, false),
                field("uid", RuntimeValueKind::Uuid, false),
                field("document", RuntimeValueKind::Json, true),
                field("happened_at", RuntimeValueKind::DateTime, false),
            ],
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![RuntimeOrderTerm {
                field: fid("rank"),
                direction: RuntimeOrderDirection::Asc,
            }],
        }],
    }
    .validate()
    .unwrap()
}

#[tokio::test]
async fn runtime_query_filters_orders_pages_counts_and_hides_cursor_fields() {
    let schema = schema();
    let collection = schema.resolve_collection(&cid("customers")).unwrap();
    let id = schema.resolve_field(&collection, &fid("id")).unwrap();
    let status = schema.resolve_field(&collection, &fid("status")).unwrap();
    let rank = schema.resolve_field(&collection, &fid("rank")).unwrap();
    let projection = schema
        .resolve_projection(&collection, std::slice::from_ref(&status))
        .unwrap();
    let limits = RuntimeQueryLimits::default();

    let database = graphql_orm::db::Database::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    graphql_orm::sqlx::query(
        "CREATE TABLE runtime_query_customers (
            id INTEGER PRIMARY KEY, status TEXT NOT NULL, rank INTEGER NULL, secret BLOB NOT NULL,
            active INTEGER NOT NULL DEFAULT 1, score REAL NOT NULL DEFAULT 1.5,
            uid TEXT NOT NULL DEFAULT '67e55044-10b1-426f-9247-bb680e5fe0c8',
            document TEXT NULL DEFAULT NULL,
            happened_at TEXT NOT NULL DEFAULT '2026-07-15T00:00:00.000000Z'
        )",
    )
    .execute(database.pool())
    .await
    .unwrap();
    for (id, status, rank) in [
        (1_i64, "inactive", Some(1_i64)),
        (2, "active", Some(2)),
        (3, "active", Some(2)),
        (4, "active", None),
        (5, "active", Some(3)),
    ] {
        graphql_orm::sqlx::query(
            "INSERT INTO runtime_query_customers (id, status, rank, secret) VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(status)
        .bind(rank)
        .bind(vec![0xff_u8, id as u8])
        .execute(database.pool())
        .await
        .unwrap();
    }

    let filter = schema
        .runtime_compare(
            &collection,
            &status,
            RuntimeScalarOperator::Eq,
            RuntimeValue::String("active".to_string()),
            limits,
        )
        .unwrap();
    let active = schema.resolve_field(&collection, &fid("active")).unwrap();
    let score = schema.resolve_field(&collection, &fid("score")).unwrap();
    let uid = schema.resolve_field(&collection, &fid("uid")).unwrap();
    let secret = schema.resolve_field(&collection, &fid("secret")).unwrap();
    let document = schema.resolve_field(&collection, &fid("document")).unwrap();
    let happened_at = schema
        .resolve_field(&collection, &fid("happened_at"))
        .unwrap();
    let all_kind_filter = schema
        .runtime_and(
            &collection,
            vec![
                schema
                    .runtime_compare(
                        &collection,
                        &active,
                        RuntimeScalarOperator::Eq,
                        RuntimeValue::Boolean(true),
                        limits,
                    )
                    .unwrap(),
                schema
                    .runtime_between(
                        &collection,
                        &score,
                        RuntimeValue::Float(RuntimeFloat::new(1.0).unwrap()),
                        RuntimeValue::Float(RuntimeFloat::new(2.0).unwrap()),
                        limits,
                    )
                    .unwrap(),
                schema
                    .runtime_list(
                        &collection,
                        &uid,
                        RuntimeListOperator::In,
                        vec![RuntimeValue::Uuid(
                            uuid::Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8").unwrap(),
                        )],
                        limits,
                    )
                    .unwrap(),
                schema
                    .runtime_compare(
                        &collection,
                        &secret,
                        RuntimeScalarOperator::Eq,
                        RuntimeValue::Bytes(vec![0xff, 2]),
                        limits,
                    )
                    .unwrap(),
                schema
                    .runtime_is_null(&collection, &document, true, limits)
                    .unwrap(),
                schema
                    .runtime_compare(
                        &collection,
                        &happened_at,
                        RuntimeScalarOperator::Gte,
                        RuntimeValue::DateTime(
                            RuntimeDateTime::parse("2026-07-14T00:00:00Z").unwrap(),
                        ),
                        limits,
                    )
                    .unwrap(),
            ],
            limits,
        )
        .unwrap();
    let all_kind_request = schema
        .runtime_read_request(
            &collection,
            &projection,
            Some(all_kind_filter),
            schema.runtime_order(&collection, None, limits).unwrap(),
            RuntimePageRequest::first(10, None),
            false,
            limits,
        )
        .unwrap();
    let all_kind_result = database
        .execute_runtime_read(&all_kind_request, None)
        .await
        .unwrap();
    assert_eq!(all_kind_result.edges.len(), 1);
    assert_eq!(
        all_kind_result.edges[0].node.string(&status).unwrap(),
        "active"
    );
    let order = schema
        .runtime_order(
            &collection,
            Some(vec![RuntimeOrderInput {
                field: rank.clone(),
                direction: RuntimeOrderDirection::Asc,
                nulls: RuntimeNullPlacement::Last,
            }]),
            limits,
        )
        .unwrap();
    assert_eq!(order.terms().len(), 2, "primary key is the tie-breaker");

    let request = schema
        .runtime_read_request(
            &collection,
            &projection,
            Some(filter.clone()),
            order.clone(),
            RuntimePageRequest::first(2, None),
            false,
            limits,
        )
        .unwrap();
    let first = database.execute_runtime_read(&request, None).await.unwrap();
    assert_eq!(first.edges.len(), 2);
    assert!(first.page_info.has_next_page);
    assert!(!first.page_info.has_previous_page);
    assert_eq!(first.total_count, None);
    assert_eq!(first.edges[0].node.string(&status).unwrap(), "active");
    assert_eq!(
        first.edges[0].node.state(&rank).unwrap(),
        RuntimeFieldState::Unloaded
    );
    assert_eq!(
        first.edges[0].node.state(&id).unwrap(),
        RuntimeFieldState::Unloaded
    );

    let request = schema
        .runtime_read_request(
            &collection,
            &projection,
            Some(filter.clone()),
            order.clone(),
            RuntimePageRequest::first(2, first.page_info.end_cursor.clone()),
            true,
            limits,
        )
        .unwrap();
    let second = database.execute_runtime_read(&request, None).await.unwrap();
    assert_eq!(second.edges.len(), 2);
    assert!(!second.page_info.has_next_page);
    assert!(second.page_info.has_previous_page);
    assert_eq!(second.total_count, Some(4));

    let back_request = schema
        .runtime_read_request(
            &collection,
            &projection,
            Some(filter),
            order,
            RuntimePageRequest::last(2, second.page_info.start_cursor.clone()),
            false,
            limits,
        )
        .unwrap();
    let backward = database
        .execute_runtime_read(&back_request, None)
        .await
        .unwrap();
    assert_eq!(
        backward
            .edges
            .iter()
            .map(|edge| edge.cursor.as_str())
            .collect::<Vec<_>>(),
        first
            .edges
            .iter()
            .map(|edge| edge.cursor.as_str())
            .collect::<Vec<_>>()
    );

    let empty_in = schema
        .runtime_list(
            &collection,
            &status,
            RuntimeListOperator::In,
            vec![],
            limits,
        )
        .unwrap();
    let empty_order = schema.runtime_order(&collection, None, limits).unwrap();
    let empty_request = schema
        .runtime_read_request(
            &collection,
            &projection,
            Some(empty_in),
            empty_order,
            RuntimePageRequest::first(10, None),
            false,
            limits,
        )
        .unwrap();
    assert!(
        database
            .execute_runtime_read(&empty_request, None)
            .await
            .unwrap()
            .edges
            .is_empty()
    );
}

#[tokio::test]
async fn runtime_query_rejects_invalid_inputs_and_tampered_cursors_before_use() {
    let schema = schema();
    let collection = schema.resolve_collection(&cid("customers")).unwrap();
    let status = schema.resolve_field(&collection, &fid("status")).unwrap();
    let projection = schema
        .resolve_projection(&collection, std::slice::from_ref(&status))
        .unwrap();
    let limits = RuntimeQueryLimits::default();
    assert_eq!(
        schema
            .runtime_compare(
                &collection,
                &status,
                RuntimeScalarOperator::Eq,
                RuntimeValue::Null,
                limits,
            )
            .unwrap_err()
            .code(),
        RuntimeQueryErrorCode::InvalidFilter
    );
    let order = schema.runtime_order(&collection, None, limits).unwrap();
    assert_eq!(
        schema
            .runtime_read_request(
                &collection,
                &projection,
                None,
                order.clone(),
                RuntimePageRequest::first(0, None),
                false,
                limits,
            )
            .unwrap_err()
            .code(),
        RuntimeQueryErrorCode::ResourceLimit
    );

    let database = graphql_orm::db::Database::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    graphql_orm::sqlx::query(
        "CREATE TABLE runtime_query_customers (
            id INTEGER PRIMARY KEY, status TEXT NOT NULL, rank INTEGER NULL, secret BLOB NOT NULL,
            active INTEGER NOT NULL DEFAULT 1, score REAL NOT NULL DEFAULT 1.5,
            uid TEXT NOT NULL DEFAULT '67e55044-10b1-426f-9247-bb680e5fe0c8',
            document TEXT NULL DEFAULT NULL,
            happened_at TEXT NOT NULL DEFAULT '2026-07-15T00:00:00.000000Z'
        )",
    )
    .execute(database.pool())
    .await
    .unwrap();
    graphql_orm::sqlx::query(
        "INSERT INTO runtime_query_customers (id, status, rank, secret)
         VALUES (1, 'active', 1, x'ff')",
    )
    .execute(database.pool())
    .await
    .unwrap();
    let request = schema
        .runtime_read_request(
            &collection,
            &projection,
            None,
            order.clone(),
            RuntimePageRequest::first(1, None),
            false,
            limits,
        )
        .unwrap();
    let cursor = database
        .execute_runtime_read(&request, None)
        .await
        .unwrap()
        .page_info
        .end_cursor
        .unwrap();
    let mut tampered = cursor.into_bytes();
    let end = tampered.len() - 1;
    tampered[end] = if tampered[end] == b'0' { b'1' } else { b'0' };
    let tampered = String::from_utf8(tampered).unwrap();
    let request = schema
        .runtime_read_request(
            &collection,
            &projection,
            None,
            order,
            RuntimePageRequest::first(1, Some(tampered)),
            false,
            limits,
        )
        .unwrap();
    assert_eq!(
        database
            .execute_runtime_read(&request, None)
            .await
            .unwrap_err()
            .code(),
        RuntimeQueryErrorCode::CursorInvalid
    );
}
