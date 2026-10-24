//! Emitted-slot admission through public reads on test-owned databases.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use graphql_orm::db::Database;
use graphql_orm::graphql::orm::*;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
fn field_id(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}
fn schema() -> ValidatedRuntimeSchema {
    RuntimeSchema {
        format_version: 1,
        collections: vec![RuntimeCollection {
            classification: None,
            description: None,
            id: CollectionId::new("budget").unwrap(),
            api_type_name: "Budget".into(),
            api_plural_name: "Budgets".into(),
            physical_table: "runtime_bind_budget".into(),
            primary_key: vec![field_id("id")],
            append_only: false,
            retention_purge: false,
            fields: [
                ("id", RuntimeValueKind::Integer),
                ("label", RuntimeValueKind::String),
            ]
            .into_iter()
            .map(|(name, kind)| RuntimeField {
                description: None,
                classification: None,
                export: None,
                id: field_id(name),
                api_name: name.into(),
                physical_column: name.into(),
                value_kind: kind,
                nullable: name == "label",
                unique: name == "id",
                filterable: true,
                sortable: true,
                generated: false,
                default: None,
                format: None,
            })
            .collect(),
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            label_fields: Vec::new(),
        }],
    }
    .validate()
    .unwrap()
}
fn request(
    s: &ValidatedRuntimeSchema,
    cap: usize,
    suffix: bool,
    page: RuntimePageRequest,
) -> RuntimeReadRequest {
    let c = s
        .resolve_collection(&CollectionId::new("budget").unwrap())
        .unwrap();
    let id = s.resolve_field(&c, &field_id("id")).unwrap();
    let label = s.resolve_field(&c, &field_id("label")).unwrap();
    let limits = RuntimeQueryLimits {
        max_bind_parameters: cap,
        ..Default::default()
    };
    let p = suffix.then(|| {
        s.runtime_compare(
            &c,
            &label,
            RuntimeScalarOperator::EndsWith,
            RuntimeValue::String("x".into()),
            limits,
        )
        .unwrap()
    });
    let order = s
        .runtime_order(
            &c,
            Some(vec![RuntimeOrderInput {
                field: label,
                direction: RuntimeOrderDirection::Asc,
                nulls: RuntimeNullPlacement::First,
            }]),
            limits,
        )
        .unwrap();
    s.runtime_read_request(
        &c,
        &s.resolve_projection(&c, &[id]).unwrap(),
        p,
        order,
        page,
        true,
        limits,
    )
    .unwrap()
}
fn ids(s: &ValidatedRuntimeSchema, connection: &RuntimeConnection) -> Vec<i64> {
    let c = s
        .resolve_collection(&CollectionId::new("budget").unwrap())
        .unwrap();
    let id = s.resolve_field(&c, &field_id("id")).unwrap();
    connection
        .edges
        .iter()
        .map(|e| match e.node.state(&id).unwrap() {
            RuntimeFieldState::Value(RuntimeValue::Integer(n)) => *n,
            other => panic!("unexpected ID state: {other:?}"),
        })
        .collect()
}
#[tokio::test]
async fn actual_owned_reads_enforce_suffix_cursor_and_count_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    for key in [
        "DATABASE_URL",
        "TEST_DATABASE_URL",
        "MSSQL_TEST_DATABASE_URL",
    ] {
        assert!(
            std::env::var_os(key).is_none(),
            "ambient database URL forbidden"
        );
    }
    #[cfg(feature = "sqlite")]
    let db = Database::<Backend>::connect_sqlite_with_options(
        "sqlite::memory:",
        graphql_orm::db::ConnectionOptions::default().max_connections(1),
    )
    .await?;
    #[cfg(feature = "postgres")]
    let mut owned = owned_postgres::OwnedPostgres::start("read-bind-budget")?;
    #[cfg(feature = "postgres")]
    let db = Database::<Backend>::connect_postgres(&owned.url).await?;
    // Result errors reach explicit cleanup below; panics rely on OwnedPostgres
    // Drop and the resource wrapper cleanup fallback.
    let result = async {
        #[cfg(feature = "sqlite")]
        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(db.pool())
            .await?;
        #[cfg(feature = "postgres")]
        let version: String = sqlx::query_scalar("SHOW server_version")
            .fetch_one(db.pool())
            .await?;
        eprintln!("OWNED_READ_BIND_BACKEND_VERSION={version}");
        sqlx::query("CREATE TABLE runtime_bind_budget(id BIGINT PRIMARY KEY, label TEXT)")
            .execute(db.pool())
            .await?;
        sqlx::query("INSERT INTO runtime_bind_budget VALUES (1,NULL),(2,'ax'),(3,'bx'),(4,'cx')")
            .execute(db.pool())
            .await?;
        let s = schema();
        let low = request(&s, 1, true, RuntimePageRequest::first(10, None));
        let result = db.execute_runtime_read(&low, None).await;
        #[cfg(feature = "sqlite")]
        assert_eq!(
            result
                .expect_err("Q019_EXPECTED_SUFFIX_RESOURCE_LIMIT")
                .code(),
            RuntimeQueryErrorCode::ResourceLimit
        );
        #[cfg(feature = "postgres")]
        {
            let page = result?;
            assert_eq!(ids(&s, &page), [2, 3, 4]);
            assert_eq!(page.total_count, Some(3));
        }
        let page = db
            .execute_runtime_read(
                &request(&s, 2, true, RuntimePageRequest::first(10, None)),
                None,
            )
            .await?;
        assert_eq!(ids(&s, &page), [2, 3, 4]);
        assert_eq!(page.total_count, Some(3));
        let first = db
            .execute_runtime_read(
                &request(&s, 3, false, RuntimePageRequest::first(2, None)),
                None,
            )
            .await?;
        assert_eq!(ids(&s, &first), [1, 2]);
        let cursor = first.page_info.end_cursor.clone().unwrap();
        let low = request(
            &s,
            2,
            false,
            RuntimePageRequest::first(2, Some(cursor.clone())),
        );
        assert_eq!(
            db.execute_runtime_read(&low, None)
                .await
                .expect_err("Q019_EXPECTED_CURSOR_RESOURCE_LIMIT")
                .code(),
            RuntimeQueryErrorCode::ResourceLimit
        );
        let next = db
            .execute_runtime_read(
                &request(&s, 3, false, RuntimePageRequest::first(2, Some(cursor))),
                None,
            )
            .await?;
        assert_eq!(ids(&s, &next), [3, 4]);
        assert_eq!(next.total_count, Some(4));
        let cursor = next.page_info.start_cursor.clone().unwrap();
        assert_eq!(
            db.execute_runtime_read(
                &request(
                    &s,
                    2,
                    false,
                    RuntimePageRequest::last(2, Some(cursor.clone()))
                ),
                None
            )
            .await
            .unwrap_err()
            .code(),
            RuntimeQueryErrorCode::ResourceLimit
        );
        let previous = db
            .execute_runtime_read(
                &request(&s, 3, false, RuntimePageRequest::last(2, Some(cursor))),
                None,
            )
            .await?;
        assert_eq!(ids(&s, &previous), [1, 2]);
        assert_eq!(previous.total_count, Some(4));
        // NULL equality/rank branches consume no slot; cap2 remains admissible.
        let null_page = db
            .execute_runtime_read(
                &request(&s, 2, false, RuntimePageRequest::first(1, None)),
                None,
            )
            .await?;
        let after_null = db
            .execute_runtime_read(
                &request(
                    &s,
                    2,
                    false,
                    RuntimePageRequest::first(2, null_page.page_info.end_cursor.clone()),
                ),
                None,
            )
            .await?;
        assert_eq!(ids(&s, &after_null), [2, 3]);
        assert_eq!(after_null.total_count, Some(4));
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    db.pool().close().await;
    #[cfg(feature = "postgres")]
    owned.cleanup()?;
    eprintln!("Q019_EXPLICIT_CLEANUP_COMPLETE");
    result
}
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn over_budget_read_rejects_before_lazy_pool_connection() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let connections = Arc::new(AtomicUsize::new(0));
    let observed = connections.clone();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .after_connect(move |_, _| {
            let observed = observed.clone();
            Box::pin(async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        })
        .connect_lazy("sqlite::memory:")
        .unwrap();
    let db = Database::<SqliteBackend>::new(pool);
    let s = schema();
    assert_eq!(
        db.execute_runtime_read(
            &request(&s, 1, true, RuntimePageRequest::first(2, None)),
            None
        )
        .await
        .unwrap_err()
        .code(),
        RuntimeQueryErrorCode::ResourceLimit
    );
    assert_eq!(connections.load(Ordering::SeqCst), 0);
    // At-bound control reaches the deliberately table-less pool: distinguishes
    // renderer admission from malformed-input/cursor or unsupported-backend errors.
    assert_eq!(
        db.execute_runtime_read(
            &request(&s, 2, true, RuntimePageRequest::first(2, None)),
            None
        )
        .await
        .unwrap_err()
        .code(),
        RuntimeQueryErrorCode::BackendExecution
    );
    assert_eq!(connections.load(Ordering::SeqCst), 1);
    db.pool().close().await;
}
