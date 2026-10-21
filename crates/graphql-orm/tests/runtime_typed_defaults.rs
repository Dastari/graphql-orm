//! Typed literal defaults: create, introspect, no-op replan (including after
//! reopen), certified runtime environment and runtime create/read semantics.
//! PostgreSQL cases own a verified disposable server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use futures::future::BoxFuture;
use graphql_orm::{graphql::errors::OrmPublicError, graphql::orm::*, prelude::*};
use std::sync::Arc;

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

const TABLE: &str = "typed_defaults";

fn datetime(text: &str) -> RuntimeDefault {
    RuntimeDefault::datetime(&RuntimeDateTime::parse_exact(text).unwrap()).unwrap()
}

/// (column, kind, nullable, default) covering every literal kind and domain edge.
fn matrix() -> Vec<(&'static str, RuntimeValueKind, bool, RuntimeDefault)> {
    use RuntimeValueKind::*;
    let uuid = uuid::Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8").unwrap();
    let json = |value: serde_json::Value| RuntimeDefault::json(&value).unwrap();
    vec![
        ("uuid_canonical", Uuid, false, RuntimeDefault::uuid(uuid)),
        // Legacy non-canonical UUID text keeps rendering exactly as stored.
        (
            "uuid_legacy_upper",
            Uuid,
            false,
            RuntimeDefault::Literal("67E55044-10B1-426F-9247-BB680E5FE0C8".into()),
        ),
        (
            "json_object",
            Json,
            false,
            json(serde_json::json!({
                "b": [1, -2, "x", true, null],
                "a": {"nested": {}},
                "quote": "it's \"q\" \\ 雪🦀\n",
                "big": u64::MAX,
                "decimal": 1.0,
                "negative": -12.5
            })),
        ),
        (
            "json_string",
            Json,
            false,
            json(serde_json::json!("plain 'text'")),
        ),
        (
            "json_null_value",
            Json,
            false,
            json(serde_json::Value::Null),
        ),
        ("json_empty_array", Json, false, json(serde_json::json!([]))),
        ("json_float", Json, false, json(serde_json::json!(0.000001))),
        (
            "json_nullable",
            Json,
            true,
            json(serde_json::json!({"k": 1})),
        ),
        (
            "bytes_empty",
            Bytes,
            false,
            RuntimeDefault::bytes(&[]).unwrap(),
        ),
        (
            "bytes_binary",
            Bytes,
            false,
            RuntimeDefault::bytes(&[0, 1, 2, 39, 92, 255]).unwrap(),
        ),
        (
            "dt_offset",
            DateTime,
            false,
            datetime("2026-10-20T22:30:00.123456+10:00"),
        ),
        ("dt_min", DateTime, false, datetime("0001-01-01T00:00:00Z")),
        (
            "dt_max",
            DateTime,
            false,
            datetime("9999-12-31T23:59:59.999999Z"),
        ),
        ("int_negative", Integer, false, RuntimeDefault::integer(-1)),
        ("int_min", Integer, false, RuntimeDefault::integer(i64::MIN)),
        ("int_max", Integer, false, RuntimeDefault::integer(i64::MAX)),
        (
            "int_above_i32",
            Integer,
            false,
            RuntimeDefault::integer(2_147_483_648),
        ),
        (
            "int_below_i32",
            Integer,
            false,
            RuntimeDefault::integer(-3_000_000_000),
        ),
        (
            "int_legacy_padded",
            Integer,
            false,
            RuntimeDefault::Literal("007".into()),
        ),
        (
            "float_negative",
            Float,
            false,
            RuntimeDefault::float(-1.5).unwrap(),
        ),
        (
            "float_integral",
            Float,
            false,
            RuntimeDefault::float(1.0).unwrap(),
        ),
        (
            "float_negative_zero",
            Float,
            false,
            RuntimeDefault::float(-0.0).unwrap(),
        ),
        (
            "float_legacy_negzero",
            Float,
            false,
            RuntimeDefault::Literal("-0.0".into()),
        ),
        (
            "float_legacy_exponent",
            Float,
            false,
            RuntimeDefault::Literal("1e5".into()),
        ),
        (
            "float_max",
            Float,
            false,
            RuntimeDefault::float(f64::MAX).unwrap(),
        ),
        (
            "float_lowest",
            Float,
            false,
            RuntimeDefault::float(f64::MIN).unwrap(),
        ),
        (
            "float_min_positive",
            Float,
            false,
            RuntimeDefault::float(f64::MIN_POSITIVE).unwrap(),
        ),
        (
            "float_tiny",
            Float,
            false,
            RuntimeDefault::float(1e-300).unwrap(),
        ),
        (
            "float_huge_negative",
            Float,
            false,
            RuntimeDefault::float(-1e300).unwrap(),
        ),
        ("string_empty", String, false, RuntimeDefault::string("")),
        (
            "string_quoted",
            String,
            false,
            RuntimeDefault::string("O'Reilly \\ 雪🦀\n\t"),
        ),
        ("bool_true", Boolean, false, RuntimeDefault::boolean(true)),
        (
            "bool_nullable",
            Boolean,
            true,
            RuntimeDefault::boolean(false),
        ),
    ]
}

fn definition() -> RuntimeSchema {
    let mut fields = vec![RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: FieldId::new("id").unwrap(),
        api_name: "id".into(),
        physical_column: "id".into(),
        value_kind: RuntimeValueKind::Integer,
        nullable: false,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
    }];
    for (column, kind, nullable, default) in matrix() {
        fields.push(RuntimeField {
            description: None,
            classification: None,
            export: None,
            id: FieldId::new(column).unwrap(),
            api_name: column.replace('_', ""),
            physical_column: column.into(),
            value_kind: kind,
            nullable,
            unique: false,
            filterable: false,
            sortable: false,
            generated: false,
            default: Some(default),
            format: None,
        });
    }
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            description: None,
            classification: None,
            id: CollectionId::new(TABLE).unwrap(),
            api_type_name: "TypedDefault".into(),
            api_plural_name: "TypedDefaults".into(),
            physical_table: TABLE.into(),
            primary_key: vec![FieldId::new("id").unwrap()],
            append_only: false,
            retention_purge: false,
            fields,
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
        }],
    }
}

struct Allow {
    projection: RuntimeProjection,
}
impl RuntimeWriteAuthority<Backend> for Allow {
    fn authorize_intent<'a>(
        &'a self,
        _check: RuntimeWriteIntent<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move { RuntimeWriteGrant::new(self.projection.clone(), None) })
    }
    fn authorize_preimage<'a>(
        &'a self,
        _check: RuntimePreimageCheck<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async { Ok(()) })
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        _tx: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        Box::pin(async move { Ok(RuntimeReturnGrant::new(check.intent.returning().cloned())) })
    }
}

struct Store {
    #[cfg(feature = "sqlite")]
    path: std::path::PathBuf,
    #[cfg(feature = "postgres")]
    owned: owned_postgres::OwnedPostgres,
}
impl Store {
    fn new() -> Self {
        Self {
            #[cfg(feature = "sqlite")]
            path: std::env::temp_dir().join(format!(
                "graphql-orm-typed-defaults-{}.db",
                uuid::Uuid::new_v4().simple()
            )),
            #[cfg(feature = "postgres")]
            owned: owned_postgres::OwnedPostgres::start("typed-defaults").unwrap(),
        }
    }
    async fn open(&self) -> Database<Backend> {
        #[cfg(feature = "sqlite")]
        let db = Database::<Backend>::connect_sqlite_with_options(
            &format!("sqlite://{}?mode=rwc", self.path.display()),
            graphql_orm::db::ConnectionOptions::default().max_connections(1),
        )
        .await
        .unwrap();
        #[cfg(feature = "postgres")]
        let db = {
            let db = Database::<Backend>::connect_postgres(&self.owned.url)
                .await
                .unwrap();
            // Prove the pool reached the test-owned database before any DDL.
            let name: String = sqlx::query_scalar("SELECT current_database()")
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert!(self.owned.url.ends_with(&format!("/{name}")));
            db
        };
        db
    }
    fn finish(self) {
        #[cfg(feature = "sqlite")]
        std::fs::remove_file(&self.path).unwrap();
        #[cfg(feature = "postgres")]
        {
            let mut owned = self.owned;
            owned.cleanup().unwrap();
        }
    }
}

async fn replan_is_empty(
    db: &Database<Backend>,
    target: &OwnedSchemaModel,
    ownership: &ManagedTableSet,
    version: &str,
) {
    let plan = db
        .schema()
        .plan_owned_migration(version, "replan", target, ownership, PlanOptions::strict())
        .await
        .unwrap();
    assert!(plan.steps().is_empty(), "{:?}", plan.statements());
    assert!(plan.statements().is_empty());
}

async fn create(
    db: &Database<Backend>,
    environment: &Arc<RuntimeMutationEnvironment>,
    request: RuntimeMutationRequest,
    authority: Allow,
) -> Result<RuntimeMutationEffect, TransactionError> {
    let environment = environment.clone();
    db.transaction(TransactionMode::StateMachine, move |tx| {
        Box::pin(async move {
            tx.mutate_runtime(&environment, &request, &authority)
                .await
                .map_err(OrmPublicError::from)
        })
    })
    .await
}

#[tokio::test]
async fn typed_literals_apply_replan_certify_and_create_actual_values() {
    let store = Store::new();
    let db = store.open().await;
    let schema = Arc::new(definition().validate().unwrap());
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let ownership = ManagedTableSet::new([TABLE.into()]).unwrap();
    let plan = db
        .schema()
        .plan_owned_migration(
            "v1",
            "typed defaults",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap();
    db.schema()
        .apply_owned_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    replan_is_empty(&db, &target, &ownership, "v2").await;
    db.pool().close().await;

    // Reopen through a fresh pool: introspection, not cached state, must agree.
    #[cfg(feature = "postgres")]
    {
        // A non-UTC database default zone must not change certification: ORM
        // sessions pin UTC. Other session zones (including seconds-precision
        // historical LMT offsets) are covered by the comparator's unit tests.
        let db = store.open().await;
        let name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(db.pool())
            .await
            .unwrap();
        sqlx::query(&format!(
            "ALTER DATABASE \"{name}\" SET timezone TO 'Asia/Kolkata'"
        ))
        .execute(db.pool())
        .await
        .unwrap();
        db.pool().close().await;
    }
    let db = store.open().await;
    #[cfg(feature = "postgres")]
    {
        let zone: String = sqlx::query_scalar("SHOW timezone")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(zone, "UTC");
    }
    replan_is_empty(&db, &target, &ownership, "v3").await;
    let environment = Arc::new(
        db.schema()
            .runtime_mutation_environment(schema.clone(), &target, &ownership)
            .await
            .unwrap(),
    );

    let collection = schema
        .resolve_collection(&CollectionId::new(TABLE).unwrap())
        .unwrap();
    let field = |name: &str| {
        schema
            .resolve_field(&collection, &FieldId::new(name).unwrap())
            .unwrap()
    };
    let mut all = vec![field("id")];
    all.extend(matrix().iter().map(|(column, ..)| field(column)));
    let projection = schema.resolve_projection(&collection, &all).unwrap();
    let allow = || Allow {
        projection: projection.clone(),
    };

    // Omitted inputs take every default and return the actual stored typed value.
    let request = schema
        .runtime_create_request(
            &collection,
            &[(field("id"), RuntimeValue::Integer(1))],
            Some(projection.clone()),
            Default::default(),
        )
        .unwrap();
    let effect = create(&db, &environment, request, allow()).await.unwrap();
    let record = effect.record().unwrap();
    for (column, kind, _, default) in matrix() {
        let RuntimeTypedDefault::Value(expected) = default.typed(kind).unwrap() else {
            unreachable!()
        };
        assert_eq!(record.value(&field(column)).unwrap(), &expected, "{column}");
    }

    // Explicit values override; explicit null follows nullability.
    let explicit = RuntimeValue::Json(serde_json::json!({"explicit": true}));
    let request = schema
        .runtime_create_request(
            &collection,
            &[
                (field("id"), RuntimeValue::Integer(2)),
                (field("json_object"), explicit.clone()),
                (field("json_nullable"), RuntimeValue::Null),
                (field("bool_nullable"), RuntimeValue::Null),
                (field("int_negative"), RuntimeValue::Integer(42)),
            ],
            Some(projection.clone()),
            Default::default(),
        )
        .unwrap();
    let effect = create(&db, &environment, request, allow()).await.unwrap();
    let record = effect.record().unwrap();
    assert_eq!(record.value(&field("json_object")).unwrap(), &explicit);
    assert!(matches!(
        record.state(&field("json_nullable")).unwrap(),
        RuntimeFieldState::Null
    ));
    assert!(matches!(
        record.state(&field("bool_nullable")).unwrap(),
        RuntimeFieldState::Null
    ));
    assert_eq!(
        record.value(&field("int_negative")).unwrap(),
        &RuntimeValue::Integer(42)
    );
    let error = schema
        .runtime_create_request(
            &collection,
            &[
                (field("id"), RuntimeValue::Integer(3)),
                (field("json_object"), RuntimeValue::Null),
            ],
            Some(projection.clone()),
            Default::default(),
        )
        .err()
        .unwrap();
    assert_eq!(error.code(), RuntimeMutationErrorCode::InvalidInput);

    // Updates never reapply create defaults.
    let key = schema
        .runtime_key(&collection, &[(field("id"), RuntimeValue::Integer(2))])
        .unwrap();
    let request = schema
        .runtime_update_request(
            key,
            &[(
                field("string_empty"),
                RuntimeValue::String("changed".into()),
            )],
            None,
            Some(projection.clone()),
            Default::default(),
        )
        .unwrap();
    let effect = create(&db, &environment, request, allow()).await.unwrap();
    let record = effect.record().unwrap();
    assert_eq!(record.value(&field("json_object")).unwrap(), &explicit);
    assert!(matches!(
        record.state(&field("json_nullable")).unwrap(),
        RuntimeFieldState::Null
    ));
    assert_eq!(
        record.value(&field("int_negative")).unwrap(),
        &RuntimeValue::Integer(42)
    );
    assert_eq!(
        record.value(&field("string_empty")).unwrap(),
        &RuntimeValue::String("changed".into())
    );
    db.pool().close().await;
    store.finish();
}

#[test]
fn every_backend_target_renders_new_kinds_without_io() {
    let schema = definition().validate().unwrap();
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let table = target
        .tables()
        .iter()
        .find(|t| t.table_name() == TABLE)
        .unwrap();
    let default = |column: &str| {
        table
            .columns()
            .iter()
            .find(|c| c.name == column)
            .unwrap()
            .default
            .clone()
            .unwrap()
    };
    #[cfg(feature = "sqlite")]
    {
        assert_eq!(default("bytes_binary"), "X'00010227 5CFF'".replace(' ', ""));
        assert_eq!(default("bytes_empty"), "X''");
        assert_eq!(default("dt_offset"), "'2026-10-20T12:30:00.123456Z'");
        assert_eq!(default("json_string"), r#"'"plain ''text''"'"#);
    }
    #[cfg(feature = "postgres")]
    {
        assert_eq!(default("bytes_binary"), r"'\x000102275cff'::bytea");
        assert_eq!(default("bytes_empty"), r"'\x'::bytea");
        assert_eq!(
            default("dt_offset"),
            "'2026-10-20T12:30:00.123456Z'::timestamptz"
        );
        assert_eq!(default("json_string"), r#"'"plain ''text''"'::jsonb"#);
    }
    // Pre-existing kinds keep their exact earlier rendering.
    assert_eq!(
        default("uuid_legacy_upper"),
        "'67E55044-10B1-426F-9247-BB680E5FE0C8'"
    );
    assert_eq!(default("int_negative"), "-1");
    assert_eq!(default("float_legacy_negzero"), "-0.0");
    assert_eq!(default("string_quoted"), "'O''Reilly \\ 雪🦀\n\t'");
}
