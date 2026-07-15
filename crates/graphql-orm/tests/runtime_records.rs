//! Public API and SQLite conformance tests for owned runtime records.

use graphql_orm::graphql::orm::{
    CollectionId, DeletePolicy, FieldId, RelationCardinality, RelationId, RelationKeyPair,
    RuntimeCollection, RuntimeDateTime, RuntimeField, RuntimeFloat, RuntimeOrderDirection,
    RuntimeOrderTerm, RuntimeRecord, RuntimeRecordErrorCode, RuntimeRelation, RuntimeSchema,
    RuntimeValue, RuntimeValueKind, ValidatedRuntimeSchema,
};
#[cfg(feature = "sqlite")]
use graphql_orm::graphql::orm::{RuntimeFieldState, RuntimeProjection};

#[cfg(feature = "sqlite")]
const _: () = assert!(
    <graphql_orm::graphql::orm::SqliteBackend as graphql_orm::graphql::orm::RuntimeRowDecoder>::RUNTIME_ROW_DECODING_SUPPORTED
);
#[cfg(feature = "mssql")]
const _: () = assert!(
    !<graphql_orm::graphql::orm::MssqlBackend as graphql_orm::graphql::orm::RuntimeRowDecoder>::RUNTIME_ROW_DECODING_SUPPORTED
);
const _: () = assert!(
    !<graphql_orm::graphql::orm::NoDefaultBackend as graphql_orm::graphql::orm::RuntimeRowDecoder>::RUNTIME_ROW_DECODING_SUPPORTED
);

fn collection_id(value: &str) -> CollectionId {
    CollectionId::new(value).expect("test collection ID")
}

fn field_id(value: &str) -> FieldId {
    FieldId::new(value).expect("test field ID")
}

fn relation_id(value: &str) -> RelationId {
    RelationId::new(value).expect("test relation ID")
}

fn field(id: &str, column: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        id: field_id(id),
        api_name: id.to_string(),
        physical_column: column.to_string(),
        value_kind: kind,
        nullable,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
    }
}

fn test_schema(table: &str) -> ValidatedRuntimeSchema {
    let customer_id = field("customer_id", "id", RuntimeValueKind::Integer, false);
    let parent_id = field(
        "customer_parent_id",
        "parent_id",
        RuntimeValueKind::Integer,
        true,
    );
    let account_id = field("account_id", "id", RuntimeValueKind::Integer, false);
    RuntimeSchema {
        format_version: 1,
        collections: vec![
            RuntimeCollection {
                id: collection_id("customers"),
                api_type_name: "Customer".to_string(),
                api_plural_name: "Customers".to_string(),
                physical_table: table.to_string(),
                primary_key: vec![customer_id.id.clone()],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    customer_id.clone(),
                    field("active", "active", RuntimeValueKind::Boolean, false),
                    field("count", "count_value", RuntimeValueKind::Integer, false),
                    field("score", "score", RuntimeValueKind::Float, false),
                    field("name", "name", RuntimeValueKind::String, false),
                    field("uid", "uid", RuntimeValueKind::Uuid, false),
                    field("document", "document", RuntimeValueKind::Json, false),
                    field("payload", "payload", RuntimeValueKind::Bytes, false),
                    field(
                        "happened_at",
                        "happened_at",
                        RuntimeValueKind::DateTime,
                        false,
                    ),
                    field("note", "note", RuntimeValueKind::String, true),
                    parent_id.clone(),
                ],
                relations: vec![RuntimeRelation {
                    id: relation_id("customer_parent"),
                    api_name: "parent".to_string(),
                    target: collection_id("accounts"),
                    key_pairs: vec![RelationKeyPair {
                        source: parent_id.id.clone(),
                        target: account_id.id.clone(),
                    }],
                    cardinality: RelationCardinality::One,
                    enforce_foreign_key: true,
                    on_delete: Some(DeletePolicy::Restrict),
                }],
                indexes: Vec::new(),
                composite_unique: Vec::new(),
                default_order: vec![RuntimeOrderTerm {
                    field: customer_id.id.clone(),
                    direction: RuntimeOrderDirection::Asc,
                }],
            },
            RuntimeCollection {
                id: collection_id("accounts"),
                api_type_name: "Account".to_string(),
                api_plural_name: "Accounts".to_string(),
                physical_table: "runtime_accounts".to_string(),
                primary_key: vec![account_id.id.clone()],
                append_only: false,
                retention_purge: false,
                fields: vec![account_id.clone()],
                relations: Vec::new(),
                indexes: Vec::new(),
                composite_unique: Vec::new(),
                default_order: vec![RuntimeOrderTerm {
                    field: account_id.id.clone(),
                    direction: RuntimeOrderDirection::Asc,
                }],
            },
        ],
    }
    .validate()
    .unwrap_or_else(|diagnostics| panic!("valid test schema: {diagnostics}"))
}

#[cfg(feature = "sqlite")]
fn customer_projection(validated: &ValidatedRuntimeSchema) -> RuntimeProjection {
    validated
        .resolve_projection_ids(
            &collection_id("customers"),
            &[
                field_id("customer_id"),
                field_id("active"),
                field_id("count"),
                field_id("score"),
                field_id("name"),
                field_id("uid"),
                field_id("document"),
                field_id("payload"),
                field_id("happened_at"),
                field_id("note"),
            ],
        )
        .expect("valid projection")
}

#[cfg(feature = "sqlite")]
fn assert_record(record: &RuntimeRecord, schema: &ValidatedRuntimeSchema) {
    let collection = schema
        .resolve_collection(&collection_id("customers"))
        .expect("customer collection");
    let resolve = |id| {
        schema
            .resolve_field(&collection, &field_id(id))
            .expect("customer field")
    };
    assert_eq!(record.integer(&resolve("customer_id")).unwrap(), i64::MAX);
    assert!(record.boolean(&resolve("active")).unwrap());
    assert_eq!(record.integer(&resolve("count")).unwrap(), i64::MIN);
    assert_eq!(record.float(&resolve("score")).unwrap(), 12.5);
    assert_eq!(record.string(&resolve("name")).unwrap(), "Māori 🦀");
    assert_eq!(
        record.uuid(&resolve("uid")).unwrap(),
        uuid::Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8").unwrap()
    );
    assert_eq!(
        record.json(&resolve("document")).unwrap(),
        &serde_json::json!({"a": [1, true], "z": "雪"})
    );
    assert_eq!(
        record.bytes(&resolve("payload")).unwrap(),
        &[0, 1, 254, 255]
    );
    assert_eq!(
        record.datetime(&resolve("happened_at")).unwrap().as_str(),
        "2026-07-15T02:34:56.123457Z"
    );
    assert_eq!(
        record.state(&resolve("note")).unwrap(),
        RuntimeFieldState::Null
    );
    assert_eq!(
        record.state(&resolve("customer_parent_id")).unwrap(),
        RuntimeFieldState::Unloaded
    );
    assert_eq!(
        record.string(&resolve("note")).unwrap_err().code(),
        RuntimeRecordErrorCode::NullValue
    );
    assert_eq!(
        record
            .integer(&resolve("customer_parent_id"))
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::FieldUnloaded
    );
    assert_eq!(
        record.string(&resolve("count")).unwrap_err().code(),
        RuntimeRecordErrorCode::WrongValueKind
    );
}

#[test]
fn runtime_values_are_owned_deterministic_and_portable() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RuntimeValue>();
    assert_send_sync::<RuntimeRecord>();

    assert_eq!(
        RuntimeFloat::new(-0.0).unwrap(),
        RuntimeFloat::new(0.0).unwrap()
    );
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            RuntimeFloat::new(value).unwrap_err().code(),
            RuntimeRecordErrorCode::InvalidValue
        );
    }

    let datetime = RuntimeDateTime::parse("2026-07-15T12:34:56.123456789+10:00").unwrap();
    assert_eq!(datetime.as_str(), "2026-07-15T02:34:56.123457Z");

    let values = vec![
        RuntimeValue::Null,
        RuntimeValue::Boolean(true),
        RuntimeValue::Integer(-42),
        RuntimeValue::Float(RuntimeFloat::new(3.5).unwrap()),
        RuntimeValue::String("owned 雪".to_string()),
        RuntimeValue::Uuid(uuid::Uuid::nil()),
        RuntimeValue::Json(serde_json::json!({"b": 2, "a": 1})),
        RuntimeValue::Bytes(vec![0, 255]),
        RuntimeValue::DateTime(datetime),
    ];
    for value in values {
        let encoded = serde_json::to_string(&value).unwrap();
        let decoded: RuntimeValue = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, value);
        assert_eq!(serde_json::to_string(&decoded).unwrap(), encoded);
    }
}

#[test]
fn validated_handles_reject_unknown_cross_collection_duplicate_and_stale_inputs() {
    let schema = test_schema("runtime_customers");
    let customers = schema
        .resolve_collection(&collection_id("customers"))
        .unwrap();
    let accounts = schema
        .resolve_collection(&collection_id("accounts"))
        .unwrap();
    assert_eq!(customers.physical_table(), "runtime_customers");
    assert_eq!(
        schema
            .resolve_collection(&collection_id("missing"))
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::UnknownCollection
    );
    assert_eq!(
        schema
            .resolve_field(&customers, &field_id("missing"))
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::UnknownField
    );

    let id = schema
        .resolve_field(&customers, &field_id("customer_id"))
        .unwrap();
    let account_id = schema
        .resolve_field(&accounts, &field_id("account_id"))
        .unwrap();
    assert_eq!(id.physical_column(), "id");
    assert_eq!(
        schema
            .resolve_projection(&customers, &[id.clone(), id.clone()])
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::DuplicateProjectionField
    );
    assert_eq!(
        schema
            .resolve_projection(&customers, &[])
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::EmptyProjection
    );
    assert_eq!(
        schema
            .resolve_projection(&customers, &[account_id])
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::CrossCollectionField
    );

    let relation = schema
        .resolve_relation(&customers, &relation_id("customer_parent"))
        .unwrap();
    assert_eq!(relation.source().id(), customers.id());
    assert_eq!(relation.target().id(), accounts.id());
    assert_eq!(relation.key_pairs().len(), 1);
    assert_eq!(
        schema
            .resolve_relation(&customers, &relation_id("missing"))
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::UnknownRelation
    );

    let changed = test_schema("runtime_customers_v2");
    let changed_customers = changed
        .resolve_collection(&collection_id("customers"))
        .unwrap();
    assert_eq!(
        changed
            .resolve_projection(&changed_customers, &[id])
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::SchemaMismatch
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_decodes_exact_projection_and_owned_record_survives_row_drop() {
    use graphql_orm::graphql::orm::SqliteBackend;
    use graphql_orm::sqlx::{Connection, Row};

    let schema = test_schema("runtime_customers");
    let projection = customer_projection(&schema);
    let mut connection = graphql_orm::sqlx::SqliteConnection::connect("sqlite::memory:")
        .await
        .unwrap();
    let row = graphql_orm::sqlx::query(
        "SELECT 9223372036854775807 AS id, 1 AS active, -9223372036854775808 AS count_value, \
         12.5 AS score, 'Māori 🦀' AS name, \
         '67e55044-10b1-426f-9247-bb680e5fe0c8' AS uid, \
         '{\"a\":[1,true],\"z\":\"雪\"}' AS document, \
         x'0001feff' AS payload, '2026-07-15T12:34:56.123456789+10:00' AS happened_at, \
         NULL AS note, 'ignored' AS unexpected_extra",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(row.len(), projection.fields().len() + 1);
    let record = projection.decode_row::<SqliteBackend>(&row).unwrap();
    drop(row);
    drop(connection);
    assert_record(&record, &schema);

    let encoded = record.to_json().unwrap();
    let decoded = RuntimeRecord::from_json(&encoded).unwrap();
    assert_eq!(decoded, record);
    assert_record(&decoded, &schema);

    let mut invalid_version = serde_json::to_value(&record).unwrap();
    invalid_version["format_version"] = serde_json::json!(2);
    let error = RuntimeRecord::from_json(&invalid_version.to_string()).unwrap_err();
    assert_eq!(error.code(), RuntimeRecordErrorCode::InvalidRecord);
    let mut wrong_kind = serde_json::to_value(&record).unwrap();
    wrong_kind["values"]["count"] = serde_json::json!({"string": "not an integer"});
    let error = RuntimeRecord::from_json(&wrong_kind.to_string()).unwrap_err();
    assert_eq!(error.code(), RuntimeRecordErrorCode::InvalidRecord);
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_decoder_fails_closed_for_missing_wrong_malformed_and_null_values() {
    use graphql_orm::graphql::orm::SqliteBackend;
    use graphql_orm::sqlx::Connection;

    let schema = test_schema("runtime_customers");
    let customers = schema
        .resolve_collection(&collection_id("customers"))
        .unwrap();
    let projection = |field_name: &str| {
        schema
            .resolve_projection_ids(&collection_id("customers"), &[field_id(field_name)])
            .unwrap()
    };
    let mut connection = graphql_orm::sqlx::SqliteConnection::connect("sqlite::memory:")
        .await
        .unwrap();

    let row = graphql_orm::sqlx::query("SELECT 1 AS another_column")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(
        projection("count")
            .decode_row::<SqliteBackend>(&row)
            .unwrap_err()
            .code(),
        RuntimeRecordErrorCode::MissingColumn
    );

    let cases = [
        (
            "count",
            "SELECT 'wrong' AS count_value",
            RuntimeRecordErrorCode::BackendTypeMismatch,
        ),
        (
            "uid",
            "SELECT 'not-a-uuid' AS uid",
            RuntimeRecordErrorCode::InvalidValue,
        ),
        (
            "document",
            "SELECT '{bad' AS document",
            RuntimeRecordErrorCode::InvalidValue,
        ),
        (
            "happened_at",
            "SELECT 'yesterday' AS happened_at",
            RuntimeRecordErrorCode::InvalidValue,
        ),
        (
            "customer_id",
            "SELECT NULL AS id",
            RuntimeRecordErrorCode::NonNullableNull,
        ),
    ];
    for (field_name, sql, expected) in cases {
        let row = graphql_orm::sqlx::query(sql)
            .fetch_one(&mut connection)
            .await
            .unwrap();
        let error = projection(field_name)
            .decode_row::<SqliteBackend>(&row)
            .unwrap_err();
        assert_eq!(error.code(), expected, "field {field_name}");
        assert_eq!(error.collection_id(), Some(customers.id()));
        assert_eq!(error.field_id(), Some(&field_id(field_name)));
        assert!(!format!("{error:?}").contains(sql));
    }
}

#[cfg(feature = "mssql")]
#[test]
fn mssql_runtime_row_decoding_is_explicitly_unsupported() {
    assert_eq!(
        RuntimeRecordErrorCode::UnsupportedBackend.as_str(),
        "unsupported_backend"
    );
}

#[test]
fn no_default_backend_runtime_row_decoding_is_explicitly_unsupported() {
    assert_eq!(
        RuntimeRecordErrorCode::UnsupportedBackend.as_str(),
        "unsupported_backend"
    );
}
