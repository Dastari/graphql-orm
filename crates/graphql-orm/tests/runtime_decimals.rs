//! Exact runtime decimals: owned migration and replans, exact round trips at
//! the definition limits, rejection without rounding, defaults, JSON records,
//! GraphQL input/output, and filter/order parity with static decimal fields.
//! PostgreSQL cases own a verified disposable server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use futures::future::BoxFuture;
use graphql_orm::rust_decimal::Decimal;
use graphql_orm::{graphql::errors::OrmPublicError, graphql::orm::*, prelude::*};
use std::sync::Arc;

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

const TABLE: &str = "ledger";

fn def(precision: u8, scale: u8) -> DecimalDef {
    DecimalDef::new(precision, scale).unwrap()
}
fn money() -> DecimalDef {
    def(12, 2)
}
fn dec(definition: DecimalDef, text: &str) -> RuntimeValue {
    RuntimeValue::Decimal(parse_runtime_decimal(definition, text).unwrap())
}

fn field(id: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        id: FieldId::new(id).unwrap(),
        api_name: id.replace('_', ""),
        physical_column: id.into(),
        value_kind: kind,
        nullable,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
        description: None,
        classification: None,
        export: None,
    }
}

/// `extra` adds a defaulted column; `amount_scale` changes one definition.
fn definition(extra: bool, amount_scale: u8) -> RuntimeSchema {
    use RuntimeValueKind::Decimal as D;
    let mut fields = vec![
        field("id", RuntimeValueKind::Integer, false),
        field("amount", D(def(12, amount_scale)), false),
        field("rate", D(def(18, 18)), true),
        field("whole", D(def(18, 0)), true),
        field("fee", D(money()), false),
        field("credit", D(money()), true),
    ];
    fields[4].default = Some(RuntimeDefault::decimal_text(money(), "1.5").unwrap());
    fields[5].default = Some(RuntimeDefault::decimal_text(money(), "-0.05").unwrap());
    if extra {
        let mut tip = field("tip", D(def(8, 3)), false);
        tip.default = Some(RuntimeDefault::decimal_text(def(8, 3), "0").unwrap());
        fields.push(tip);
    }
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            id: CollectionId::new(TABLE).unwrap(),
            api_type_name: "Ledger".into(),
            api_plural_name: "Ledgers".into(),
            physical_table: TABLE.into(),
            primary_key: vec![FieldId::new("id").unwrap()],
            append_only: false,
            retention_purge: false,
            fields,
            relations: vec![],
            indexes: vec![],
            composite_unique: vec![],
            default_order: vec![],
            description: None,
            classification: None,
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
    fn new(label: &str) -> Self {
        Self {
            #[cfg(feature = "sqlite")]
            path: std::env::temp_dir().join(format!(
                "graphql-orm-{label}-{}.db",
                uuid::Uuid::new_v4().simple()
            )),
            #[cfg(feature = "postgres")]
            owned: owned_postgres::OwnedPostgres::start(label).unwrap(),
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

async fn migrate(
    db: &Database<Backend>,
    schema: &ValidatedRuntimeSchema,
    version: &str,
) -> (OwnedSchemaModel, ManagedTableSet, Vec<String>) {
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let ownership = ManagedTableSet::new([TABLE.into()]).unwrap();
    let plan = db
        .schema()
        .plan_owned_migration(
            version,
            "decimals",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap();
    let statements = plan.statements().to_vec();
    db.schema()
        .apply_owned_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    (target, ownership, statements)
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
}

async fn write(
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

const COLUMN_TYPE: &str = if cfg!(feature = "sqlite") {
    "INTEGER_DECIMAL(12,2)"
} else {
    "NUMERIC(12,2)"
};

#[test]
fn decimal_kind_identity_validation_and_defaults() {
    let kind = RuntimeValueKind::Decimal(money());
    assert_eq!(
        serde_json::to_string(&kind).unwrap(),
        r#"{"decimal":{"precision":12,"scale":2}}"#
    );
    assert_eq!(
        serde_json::to_string(&RuntimeValueKind::Integer).unwrap(),
        r#""integer""#
    );
    assert!(serde_json::from_str::<RuntimeValueKind>(r#""decimal""#).is_err());
    assert!(
        serde_json::from_str::<RuntimeValueKind>(r#"{"decimal":{"precision":19,"scale":2}}"#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<RuntimeValueKind>(r#"{"decimal":{"precision":2,"scale":3}}"#)
            .is_err()
    );
    let schema = definition(false, 2).validate().unwrap();
    let canonical = String::from_utf8(schema.canonical_bytes()).unwrap();
    assert!(canonical.contains("|kind=decimal(12,2)|"));
    assert!(canonical.contains("|kind=decimal(18,18)|"));
    assert!(canonical.contains("|default=literal:\"1.50\"\n"));
    assert!(canonical.contains("|default=literal:\"-0.05\"\n"));
    assert_ne!(
        schema.structural_fingerprint(),
        definition(false, 3)
            .validate()
            .unwrap()
            .structural_fingerprint()
    );

    // Defaults: canonical literals only; the text constructor normalizes losslessly.
    assert_eq!(
        RuntimeDefault::decimal_text(money(), "12.5").unwrap(),
        RuntimeDefault::Literal("12.50".into())
    );
    assert_eq!(
        RuntimeDefault::decimal(parse_runtime_decimal(money(), "-0").unwrap()),
        RuntimeDefault::Literal("0.00".into())
    );
    for (text, problem) in [
        ("12.345", RuntimeDefaultProblem::UnsupportedPrecision),
        ("10000000000", RuntimeDefaultProblem::OutOfRange),
        ("1e2", RuntimeDefaultProblem::Malformed),
    ] {
        assert_eq!(
            RuntimeDefault::decimal_text(money(), text)
                .unwrap_err()
                .problem(),
            problem,
            "{text}"
        );
    }
    let mut bad = definition(false, 2);
    for literal in ["12.5", "-0.00", "+1.00", "1.500"] {
        bad.collections[0].fields[4].default = Some(RuntimeDefault::Literal(literal.into()));
        let diagnostics = bad.clone().validate().unwrap_err();
        let diagnostic = &diagnostics.diagnostics()[0];
        assert_eq!(diagnostic.code, RuntimeSchemaDiagnosticCode::InvalidDefault);
        assert!(!diagnostic.message.contains(literal));
    }
    bad.collections[0].fields[4].default = Some(RuntimeDefault::CurrentTimestamp);
    assert!(bad.clone().validate().is_err());
    assert_eq!(
        RuntimeDefault::Literal("1.50".into())
            .typed(RuntimeValueKind::Decimal(money()))
            .unwrap(),
        RuntimeTypedDefault::Value(dec(money(), "1.50"))
    );

    // No decimal keys or generated decimals.
    let unsupported = |mutate: fn(&mut RuntimeSchema)| {
        let mut schema = definition(false, 2);
        mutate(&mut schema);
        schema
            .validate()
            .unwrap_err()
            .diagnostics()
            .iter()
            .any(|d| d.code == RuntimeSchemaDiagnosticCode::UnsupportedCapability)
    };
    assert!(unsupported(
        |s| s.collections[0].primary_key = vec![FieldId::new("amount").unwrap()]
    ));
    assert!(unsupported(|s| s.collections[0].fields[1].generated = true));
    assert!(unsupported(|s| {
        s.collections[0].relations.push(RuntimeRelation {
            id: RelationId::new("ledger.self").unwrap(),
            api_name: "same".into(),
            target: CollectionId::new(TABLE).unwrap(),
            key_pairs: vec![RelationKeyPair {
                source: FieldId::new("fee").unwrap(),
                target: FieldId::new("amount").unwrap(),
            }],
            cardinality: RelationCardinality::One,
            enforce_foreign_key: false,
            on_delete: None,
            description: None,
            classification: None,
            export: None,
        })
    }));
}

/// Static decimal metadata converts to the runtime kind describing its storage.
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "converted_ledger", plural = "ConvertedLedgers")]
struct ConvertedLedger {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[graphql_orm(decimal(precision = 12, scale = 2), default = "1.25")]
    amount: Decimal,
    #[graphql_orm(decimal(precision = 6, scale = 0), default = "-7")]
    units: Option<Decimal>,
}

#[test]
fn static_decimal_metadata_converts_to_runtime_decimals() {
    let schema = RuntimeSchema::from_static_entities(&[ConvertedLedger::metadata()])
        .unwrap()
        .validate()
        .unwrap();
    let fields = &schema.schema().collections[0].fields;
    let field = |name: &str| fields.iter().find(|f| f.physical_column == name).unwrap();
    assert_eq!(
        field("amount").value_kind,
        RuntimeValueKind::Decimal(money())
    );
    assert_eq!(
        field("amount").default,
        Some(RuntimeDefault::Literal("1.25".into()))
    );
    assert_eq!(
        field("units").value_kind,
        RuntimeValueKind::Decimal(def(6, 0))
    );
    assert_eq!(
        field("units").default,
        Some(RuntimeDefault::Literal("-7".into()))
    );
}

#[test]
fn json_records_and_values_carry_canonical_strings() {
    let value = dec(money(), "12.5");
    let json = serde_json::to_value(&value).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"decimal": {"value": "12.50", "definition": {"precision": 12, "scale": 2}}})
    );
    assert_eq!(serde_json::from_value::<RuntimeValue>(json).unwrap(), value);
    let parse = |value: serde_json::Value| {
        serde_json::from_value::<RuntimeValue>(serde_json::json!({
            "decimal": {"value": value, "definition": {"precision": 12, "scale": 2}}
        }))
    };
    // The same strict inputs as the GraphQL scalar: decimal text or an integer.
    assert_eq!(parse("12.5".into()).unwrap(), value);
    assert_eq!(parse(12.into()).unwrap(), dec(money(), "12"));
    for rejected in [
        serde_json::json!(12.5),
        "12.345".into(),
        "1e1".into(),
        " 1".into(),
        "+1".into(),
        "10000000000".into(),
    ] {
        assert!(parse(rejected.clone()).is_err(), "{rejected}");
    }
}

#[tokio::test]
async fn decimals_migrate_round_trip_exactly_and_replan_as_no_ops() {
    let store = Store::new("runtime-decimals");
    let db = store.open().await;
    let schema = Arc::new(definition(false, 2).validate().unwrap());
    let (target, ownership, statements) = migrate(&db, &schema, "v1").await;
    assert!(
        statements.iter().any(|sql| sql.contains(COLUMN_TYPE)),
        "{statements:?}"
    );
    replan_is_empty(&db, &target, &ownership, "v1-replan").await;
    db.pool().close().await;
    let db = store.open().await;
    replan_is_empty(&db, &target, &ownership, "v1-reopen").await;
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
    let all = ["id", "amount", "rate", "whole", "fee", "credit"].map(field);
    let projection = schema.resolve_projection(&collection, &all).unwrap();
    let allow = || Allow {
        projection: projection.clone(),
    };
    let create = |id: i64, values: Vec<(&str, RuntimeValue)>| {
        let mut inputs = vec![(field("id"), RuntimeValue::Integer(id))];
        inputs.extend(values.into_iter().map(|(name, value)| (field(name), value)));
        schema.runtime_create_request(
            &collection,
            &inputs,
            Some(projection.clone()),
            Default::default(),
        )
    };

    // Limits round-trip exactly; omitted inputs take the decimal defaults.
    let extremes = [
        (
            "9999999999.99",
            "0.999999999999999999",
            "999999999999999999",
        ),
        (
            "-9999999999.99",
            "-0.999999999999999999",
            "-999999999999999999",
        ),
        ("0.01", "0.000000000000000001", "0"),
    ];
    for (id, (amount, rate, whole)) in extremes.iter().enumerate() {
        let request = create(
            id as i64 + 1,
            vec![
                ("amount", dec(money(), amount)),
                ("rate", dec(def(18, 18), rate)),
                ("whole", dec(def(18, 0), whole)),
            ],
        )
        .unwrap();
        let effect = write(&db, &environment, request, allow()).await.unwrap();
        let record = effect.record().unwrap();
        for (name, text) in [("amount", amount), ("rate", rate), ("whole", whole)] {
            let value = record.decimal_value(&field(name)).unwrap();
            assert_eq!(canonical_runtime_decimal(value), *text, "{name}");
        }
        assert_eq!(record.decimal(&field("fee")).unwrap(), Decimal::new(150, 2));
        assert_eq!(
            record.decimal(&field("credit")).unwrap(),
            Decimal::new(-5, 2)
        );
        assert_eq!(
            record.integer(&field("amount")).unwrap_err().code(),
            RuntimeRecordErrorCode::WrongValueKind
        );
        // JSON records carry canonical strings and reload identically.
        let json = serde_json::to_string(record).unwrap();
        assert!(json.contains(&format!("\"value\":\"{amount}\"")));
        assert_eq!(
            &serde_json::from_str::<RuntimeRecord>(&json).unwrap(),
            record
        );
    }
    // A value whose definition differs from the field's is a different kind.
    let wrong = RuntimeValue::Decimal(parse_runtime_decimal(def(12, 3), "1.000").unwrap());
    assert_eq!(
        create(9, vec![("amount", wrong)]).err().unwrap().code(),
        RuntimeMutationErrorCode::InvalidInput
    );
    // Values outside the definition are unrepresentable, never rounded.
    assert_eq!(
        parse_runtime_decimal(money(), "1.005")
            .unwrap_err()
            .problem(),
        RuntimeDecimalProblem::Scale
    );
    assert_eq!(
        parse_runtime_decimal(money(), "10000000000")
            .unwrap_err()
            .problem(),
        RuntimeDecimalProblem::Precision
    );

    // A stored mantissa beyond the definition is rejected when read.
    #[cfg(feature = "sqlite")]
    {
        sqlx::query("INSERT INTO ledger (id, amount, fee) VALUES (99, 1000000000000, 150)")
            .execute(db.pool())
            .await
            .unwrap();
        let key = schema
            .runtime_key(&collection, &[(field("id"), RuntimeValue::Integer(99))])
            .unwrap();
        let request = schema
            .runtime_update_request(
                key,
                &[(field("whole"), dec(def(18, 0), "1"))],
                None,
                Some(projection.clone()),
                Default::default(),
            )
            .unwrap();
        assert!(write(&db, &environment, request, allow()).await.is_err());
    }

    // AddColumn with a decimal default, then a no-op replan.
    let extended = Arc::new(definition(true, 2).validate().unwrap());
    let (target, ownership, statements) = migrate(&db, &extended, "v2").await;
    let tip = if cfg!(feature = "sqlite") {
        "INTEGER_DECIMAL(8,3)"
    } else {
        "NUMERIC(8,3)"
    };
    assert!(
        statements
            .iter()
            .any(|sql| sql.contains("ADD COLUMN") && sql.contains(tip)),
        "{statements:?}"
    );
    replan_is_empty(&db, &target, &ownership, "v2-replan").await;
    db.schema()
        .runtime_mutation_environment(extended.clone(), &target, &ownership)
        .await
        .unwrap();

    // A definition change is detected and refused: it is never applied in place.
    let rescaled = definition(true, 3).validate().unwrap();
    let target = rescaled
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let refused = db
        .schema()
        .plan_owned_migration("v3", "rescale", &target, &ownership, PlanOptions::strict())
        .await
        .err()
        .unwrap();
    let RuntimeMigrationError::Diagnostics(diagnostics) = refused else {
        panic!("{refused}")
    };
    assert_eq!(
        diagnostics.0[0].code,
        RuntimeMigrationDiagnosticCode::InvalidPhysicalContract
    );
    assert!(
        db.schema()
            .runtime_mutation_environment(Arc::new(rescaled), &target, &ownership)
            .await
            .is_err()
    );
    db.pool().close().await;
    store.finish();
}

#[cfg(feature = "runtime-graphql")]
mod graphql {
    use super::*;
    use graphql_orm::async_graphql::{Request, Variables, dynamic::Schema};
    use graphql_orm::graphql::runtime::*;
    use serde_json::{Value, json};

    /// Static decimal entity with the same definition, for parity.
    #[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[repository_entity(
        table = "static_ledger",
        plural = "StaticLedgers",
        default_sort = "id ASC"
    )]
    struct StaticLedger {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        id: String,
        #[filterable(type = "decimal")]
        #[sortable]
        #[graphql_orm(decimal(precision = 12, scale = 2))]
        amount: Decimal,
    }

    struct Read;
    impl RuntimeReadAuthority for Read {
        fn authorize_mutation_result<'a>(
            &'a self,
            _check: RuntimeMutationOutputCheck<'a>,
        ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
            Box::pin(async { Ok(()) })
        }
        fn authorize<'a>(
            &'a self,
            _check: RuntimeReadCheck<'a>,
        ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>> {
            Box::pin(async { Ok(RuntimeReadGrant::new(None)) })
        }
    }

    struct Api {
        schema: Arc<ValidatedRuntimeSchema>,
        environment: Arc<RuntimeMutationEnvironment>,
        projection: RuntimeProjection,
        api: Schema,
    }
    impl Api {
        async fn execute(&self, query: &str, variables: Value) -> Value {
            let mut variables = variables;
            variables["schema"] = json!(self.schema.fingerprint().to_string());
            let response = self
                .api
                .execute(
                    Request::new(query)
                        .variables(Variables::from_json(variables))
                        .data(
                            RuntimeGraphqlRequest::<Backend>::new(
                                self.schema.fingerprint(),
                                Arc::new(Read),
                                None,
                                Default::default(),
                            )
                            .with_mutation_environment(self.environment.clone())
                            .with_write_authority(Arc::new(Allow {
                                projection: self.projection.clone(),
                            })),
                        ),
                )
                .await;
            serde_json::to_value(&response).unwrap()
        }
    }

    async fn api(db: &Database<Backend>) -> Api {
        let schema = Arc::new(definition(false, 2).validate().unwrap());
        let (target, ownership, _) = migrate(db, &schema, "v1").await;
        let environment = Arc::new(
            db.schema()
                .runtime_mutation_environment(schema.clone(), &target, &ownership)
                .await
                .unwrap(),
        );
        let collection = schema
            .resolve_collection(&CollectionId::new(TABLE).unwrap())
            .unwrap();
        let fields = ["id", "amount", "rate", "whole", "fee", "credit"].map(|name| {
            schema
                .resolve_field(&collection, &FieldId::new(name).unwrap())
                .unwrap()
        });
        let projection = schema.resolve_projection(&collection, &fields).unwrap();
        let module = RuntimeGraphqlModule::compile(
            schema.clone(),
            RuntimeGraphqlOptions::default()
                .with_cursor_profile(RuntimeCursorProfile::Unprotected)
                .with_mutations(Default::default()),
        )
        .unwrap();
        let api = RuntimeGraphqlComposer::new(db.clone(), "Query", Default::default())
            .unwrap()
            .mutation_root("Mutation")
            .unwrap()
            .install(module)
            .unwrap()
            .finish()
            .unwrap();
        Api {
            schema,
            environment,
            projection,
            api,
        }
    }

    const CREATE: &str = r#"mutation($schema: String!, $input: CreateLedgerInput!) {
        createLedger(expectedSchema: $schema, input: $input) { record { id amount fee credit } }
    }"#;

    #[tokio::test]
    async fn graphql_decimals_are_exact_strings_and_never_rounded() {
        let store = Store::new("runtime-decimal-graphql");
        let db = store.open().await;
        let api = api(&db).await;
        let sdl = api.api.sdl();
        assert!(sdl.contains("scalar Decimal"));
        assert!(sdl.contains("amount: Decimal!"));
        assert!(sdl.contains("input RuntimeDecimalFilter"));
        // The catalogue advertises the same scalar and exact comparisons; there
        // are no runtime aggregates.
        let module = RuntimeGraphqlModule::compile(
            api.schema.clone(),
            RuntimeGraphqlOptions::default().with_cursor_profile(RuntimeCursorProfile::Unprotected),
        )
        .unwrap();
        let entity = &module.semantic_entities()[0];
        let amount = entity
            .fields
            .iter()
            .find(|f| f.field_name == "amount")
            .unwrap();
        assert_eq!(
            amount.type_ref,
            GraphqlSemanticTypeRef::named("Decimal", GraphqlSemanticTypeKind::Scalar, false)
        );
        assert!(
            amount
                .filter_operators
                .contains(&GraphqlSemanticFilterOperator::LessThan)
        );
        assert!(amount.aggregate_operators.is_empty() && amount.aggregate_value_kind.is_none());

        let created = api
            .execute(CREATE, json!({"input": {"id": "1", "amount": "12.5"}}))
            .await;
        assert_eq!(
            created["data"]["createLedger"]["record"],
            json!({"id": "1", "amount": "12.50", "fee": "1.50", "credit": "-0.05"})
        );
        let integer = api
            .execute(CREATE, json!({"input": {"id": "2", "amount": 7}}))
            .await;
        assert_eq!(integer["data"]["createLedger"]["record"]["amount"], "7.00");
        for rejected in [
            json!(1.25),
            json!("1.255"),
            json!("10000000000"),
            json!("1e2"),
            json!("+1"),
            json!(" 1"),
        ] {
            let response = api
                .execute(CREATE, json!({"input": {"id": "3", "amount": rejected}}))
                .await;
            assert!(
                response["errors"].as_array().is_some_and(|e| !e.is_empty()),
                "{rejected}: {response}"
            );
        }
        let filter = api
            .execute(
                r#"query { ledgers(where: { amount: { lt: "12.345" } }) { totalCount } }"#,
                json!({}),
            )
            .await;
        assert!(filter["errors"].as_array().is_some_and(|e| !e.is_empty()));
        db.pool().close().await;
        store.finish();
    }

    #[tokio::test]
    async fn filters_and_ordering_match_static_decimal_fields() {
        let store = Store::new("runtime-decimal-parity");
        let db = store.open().await;
        // The static table first: static planning owns every table it knows.
        let plan = db
            .schema()
            .plan_migration_to_entities("static", "static ledger", &[StaticLedger::metadata()])
            .await
            .unwrap();
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .unwrap();
        let api = api(&db).await;
        let amounts = [
            "-3.10",
            "0.00",
            "0.05",
            "12.50",
            "-0.05",
            "9999999999.99",
            "2.00",
        ];
        for (id, amount) in amounts.iter().enumerate() {
            let id = id as i64 + 1;
            let response = api
                .execute(
                    CREATE,
                    json!({"input": {"id": id.to_string(), "amount": amount}}),
                )
                .await;
            assert!(response.get("errors").is_none(), "{response}");
            StaticLedger::insert(
                &db,
                CreateStaticLedgerInput {
                    id: id.to_string(),
                    amount: amount.parse().unwrap(),
                },
            )
            .await
            .unwrap();
        }

        let runtime = |query: String| {
            let api = &api;
            async move {
                let response = api.execute(&query, json!({})).await;
                assert!(response.get("errors").is_none(), "{response}");
                response["data"]["ledgers"]["edges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|edge| {
                        (
                            edge["node"]["id"].as_str().unwrap().to_owned(),
                            edge["node"]["amount"].as_str().unwrap().to_owned(),
                        )
                    })
                    .collect::<Vec<_>>()
            }
        };
        // Parity is by exact value. Runtime output is always canonical; static
        // PostgreSQL reads may drop the scale of zero (`0` rather than `0.00`).
        let statics = |rows: Vec<StaticLedger>| {
            rows.into_iter()
                .map(|row| {
                    let value = parse_runtime_decimal(money(), &row.amount.to_string()).unwrap();
                    (row.id, canonical_runtime_decimal(value))
                })
                .collect::<Vec<_>>()
        };
        for direction in [OrderDirection::Asc, OrderDirection::Desc] {
            let expected = statics(
                StaticLedger::query(&db)
                    .filter(StaticLedgerWhereInput {
                        amount: Some(DecimalFilter {
                            gte: Some(Decimal::new(-5, 2)),
                            lt: Some(Decimal::new(1250, 2)),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .order_by(StaticLedgerOrderByInput {
                        amount: Some(direction),
                        ..Default::default()
                    })
                    .fetch_all()
                    .await
                    .unwrap(),
            );
            let token = if direction == OrderDirection::Asc {
                "ASC"
            } else {
                "DESC"
            };
            let actual = runtime(format!(
                r#"{{ ledgers(where: {{ amount: {{ gte: "-0.05", lt: "12.50" }} }},
                    orderBy: [{{ field: amount, direction: {token} }}]) {{
                    edges {{ node {{ id amount }} }} }} }}"#
            ))
            .await;
            assert_eq!(actual, expected, "{token}");
            assert_eq!(actual.len(), 4);
        }
        let ordered = runtime(
            r#"{ ledgers(orderBy: [{ field: amount }]) { edges { node { id amount } } } }"#.into(),
        )
        .await;
        let expected = statics(
            StaticLedger::query(&db)
                .order_by(StaticLedgerOrderByInput {
                    amount: Some(OrderDirection::Asc),
                    ..Default::default()
                })
                .fetch_all()
                .await
                .unwrap(),
        );
        assert_eq!(ordered, expected);
        assert_eq!(ordered.first().unwrap().1, "-3.10");
        assert_eq!(ordered.last().unwrap().1, "9999999999.99");
        let equal = runtime(
            r#"{ ledgers(where: { amount: { in: ["0", "2"], ne: "0.00" } }) { edges { node { id amount } } } }"#
                .into(),
        )
        .await;
        assert_eq!(equal, [("7".to_owned(), "2.00".to_owned())]);

        // Keyset cursors over decimals resume exactly.
        let first = api
            .execute(
                r#"{ ledgers(first: 3, orderBy: [{ field: amount }]) { edges { cursor node { id } } } }"#,
                json!({}),
            )
            .await;
        let cursor = first["data"]["ledgers"]["edges"][2]["cursor"]
            .as_str()
            .unwrap()
            .to_owned();
        let rest = runtime(format!(
            r#"{{ ledgers(first: 10, after: "{cursor}", orderBy: [{{ field: amount }}]) {{ edges {{ node {{ id amount }} }} }} }}"#
        ))
        .await;
        assert_eq!(rest, expected[3..].to_vec());
        db.pool().close().await;
        store.finish();
    }
}
