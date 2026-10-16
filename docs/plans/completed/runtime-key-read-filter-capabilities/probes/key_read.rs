//! Public A-only consumer; no direct GraphQL, driver or framework dependency.
use graphql_orm::{db::Database, graphql::orm::*};
#[cfg(feature = "sqlite")]
type SelectedBackend = SqliteBackend;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
type SelectedBackend = PostgresBackend;
#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
type SelectedBackend = MssqlBackend;

fn request() -> RuntimeKeyReadRequest {
    let field = |name: &str| RuntimeField {
        id: FieldId::new(name).unwrap(), api_name: name.into(), physical_column: name.into(),
        value_kind: RuntimeValueKind::Integer, nullable: false, unique: false,
        filterable: false, sortable: false, generated: false, default: None,
    };
    let schema = RuntimeSchema { format_version: RUNTIME_SCHEMA_FORMAT_VERSION, collections: vec![RuntimeCollection {
        id: CollectionId::new("rows").unwrap(), api_type_name: "Row".into(), api_plural_name: "Rows".into(),
        physical_table: "rows".into(), primary_key: vec![FieldId::new("id").unwrap()], append_only: false, retention_purge: false,
        fields: vec![field("id"), field("value")], relations: vec![], indexes: vec![], composite_unique: vec![], default_order: vec![],
    }] }.validate().unwrap();
    let collection = schema.resolve_collection(&CollectionId::new("rows").unwrap()).unwrap();
    let id = schema.resolve_field(&collection, &FieldId::new("id").unwrap()).unwrap();
    let value = schema.resolve_field(&collection, &FieldId::new("value").unwrap()).unwrap();
    let key = schema.runtime_key(&collection, &[(id, RuntimeValue::Integer(1))]).unwrap();
    let projection = schema.resolve_projection(&collection, &[value]).unwrap();
    let required = schema.runtime_and(&collection, vec![], Default::default()).unwrap();
    schema.runtime_key_read_request(&key, &projection, &required, None, Default::default()).unwrap()
}

#[allow(dead_code)]
async fn execute<B: OrmBackend + RuntimeRowDecoder>(db: &Database<B>, request: &RuntimeKeyReadRequest)
    -> Result<Option<RuntimeRecord>, RuntimeKeyReadError> {
    db.execute_runtime_key_read(request, None).await
}

#[test]
fn owned_request_remains_send_sync_after_schema_and_caller_handles_drop() {
    fn send_sync_static<T: Send + Sync + 'static>(_: &T) {}
    let request = request();
    send_sync_static(&request);
    let _selected_executor = execute::<SelectedBackend>;
    assert!(format!("{request:?}").contains("redacted"));
}

#[test]
fn legacy_exhaustive_query_error_match_still_compiles() {
    fn legacy(code: RuntimeQueryErrorCode) -> &'static str {
        match code {
            RuntimeQueryErrorCode::InvalidHandle => "invalid_handle",
            RuntimeQueryErrorCode::InvalidRequest => "invalid_request",
            RuntimeQueryErrorCode::InvalidFilter => "invalid_filter",
            RuntimeQueryErrorCode::UnsupportedOperator => "unsupported_operator",
            RuntimeQueryErrorCode::UnsupportedOrder => "unsupported_order",
            RuntimeQueryErrorCode::UnsupportedBackend => "unsupported_backend",
            RuntimeQueryErrorCode::ResourceLimit => "resource_limit",
            RuntimeQueryErrorCode::CursorInvalid => "cursor_invalid",
            RuntimeQueryErrorCode::CursorSchemaMismatch => "cursor_schema_mismatch",
            RuntimeQueryErrorCode::Decode => "decode",
            RuntimeQueryErrorCode::BackendExecution => "backend_execution",
        }
    }
    assert_eq!(legacy(RuntimeQueryErrorCode::UnsupportedBackend), "unsupported_backend");
}
