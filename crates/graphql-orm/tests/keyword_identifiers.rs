//! Tables, columns and indexes named with SQL reserved words work end to end:
//! generated DDL and DML delimit them, introspection round-trips them, and
//! re-plans stay empty. PostgreSQL cases own a verified disposable server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use futures::future::BoxFuture;
use graphql_orm::{graphql::errors::OrmPublicError, graphql::orm::*, prelude::*};
use std::sync::Arc;

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;

/// A derived entity whose table and ordinary columns are reserved words.
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "order",
    plural = "Orders",
    default_sort = "id ASC",
    index = "join"
)]
struct Order {
    #[primary_key]
    #[filterable(type = "uuid")]
    #[sortable]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub join: String,

    #[filterable(type = "string")]
    #[sortable]
    pub select: Option<String>,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "keyword_primary_keys",
    plural = "KeywordPrimaryKeys",
    auth = "none"
)]
struct KeywordPrimaryKey {
    #[primary_key]
    #[graphql_orm(db_column = "join")]
    #[filterable(type = "uuid")]
    #[sortable]
    pub id: graphql_orm::uuid::Uuid,
    pub value: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [Order, KeywordPrimaryKey],
}

#[cfg(feature = "sqlite")]
async fn database(_label: &str) -> Database<Backend> {
    Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap()
}

#[cfg(feature = "postgres")]
async fn database(label: &str) -> Database<Backend> {
    let pool = owned_postgres::OwnedPostgres::pool(label, 5).await.unwrap();
    Database::<PostgresBackend>::new(pool)
}

#[cfg(feature = "sqlite")]
async fn introspect(database: &Database<Backend>) -> SchemaModel {
    introspect_sqlite_schema(database).await.unwrap()
}

#[cfg(feature = "postgres")]
async fn introspect(database: &Database<Backend>) -> SchemaModel {
    introspect_postgres_schema(database).await.unwrap()
}

#[test]
fn generated_ddl_delimits_only_reserved_identifiers() {
    let entities = [Order::metadata()];
    let target = SchemaModel::from_entities(&entities);
    let backend = <Backend as OrmBackend>::DIALECT;
    let table = target
        .tables
        .iter()
        .find(|table| table.table_name == "order")
        .unwrap();
    let statements = render_migration_step(backend, &MigrationStep::CreateTable(table.clone()));
    let create = &statements[0];
    assert!(
        create.starts_with("CREATE TABLE \"order\" (id "),
        "{create}"
    );
    assert!(create.contains("\"join\" TEXT NOT NULL"), "{create}");
    assert!(create.contains("\"select\" TEXT"), "{create}");
    assert!(
        statements
            .iter()
            .any(|sql| sql.contains("ON \"order\"") && sql.contains("(\"join\")")),
        "{statements:?}"
    );
}

#[tokio::test]
async fn derived_entities_with_reserved_names_round_trip_and_serve_crud() {
    let database = database("keyword-identifiers-derived").await;
    let entities = [Order::metadata()];
    let plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            "keyword-v1",
            "reserved names",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await
        .unwrap();
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();

    // Introspection reads the undelimited names back, so the re-plan is empty.
    let live = introspect(&database).await;
    let table = live
        .tables
        .iter()
        .find(|table| table.table_name == "order")
        .expect("the reserved-word table exists");
    let columns = table
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        columns.contains(&"join") && columns.contains(&"select"),
        "{columns:?}"
    );
    assert!(
        table
            .indexes
            .iter()
            .any(|index| index.columns == ["join".to_string()]),
        "{:?}",
        table.indexes
    );
    let replan = database
        .schema()
        .plan_migration_to_entities_with_options(
            "keyword-v2",
            "replan",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await
        .unwrap();
    assert!(replan.steps.is_empty(), "{:?}", replan.statements);

    let alpha = Order::insert(
        &database,
        CreateOrderInput {
            join: "inner".into(),
            select: Some("b".into()),
        },
    )
    .await
    .unwrap();
    let beta = Order::insert(
        &database,
        CreateOrderInput {
            join: "inner".into(),
            select: Some("a".into()),
        },
    )
    .await
    .unwrap();
    Order::insert(
        &database,
        CreateOrderInput {
            join: "outer".into(),
            select: None,
        },
    )
    .await
    .unwrap();

    let inner = || OrderWhereInput {
        join: Some(StringFilter {
            eq: Some("inner".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let rows = Order::query(database.pool())
        .filter(inner())
        .order_by(OrderOrderByInput {
            select: Some(OrderDirection::Asc),
            ..Default::default()
        })
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        [beta.id, alpha.id]
    );

    Order::update_by_id(
        &database,
        &alpha.id,
        UpdateOrderInput {
            select: Some(Some("c".into())),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let updated = Order::find_by_id(&database, &alpha.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.select.as_deref(), Some("c"));

    assert!(Order::delete_by_id(&database, &beta.id).await.unwrap());
    assert_eq!(
        Order::count_query(database.pool())
            .filter(&inner())
            .count()
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        Order::query(database.pool())
            .fetch_all()
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn keyword_primary_keys_support_repository_and_graphql_crud() {
    let database = database("keyword-primary-keys").await;
    let entities = [KeywordPrimaryKey::metadata()];
    let plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            "keyword-primary-key-v1",
            "reserved primary key",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await
        .unwrap();
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    assert_eq!(KeywordPrimaryKey::PRIMARY_KEY, "join");

    // The pool-based insert reread and the update/delete preimages all use
    // the physical primary key, while its metadata identity remains raw.
    let row = KeywordPrimaryKey::insert(
        database.pool(),
        CreateKeywordPrimaryKeyInput {
            value: "repository".into(),
        },
    )
    .await
    .unwrap();
    let updated = KeywordPrimaryKey::update_by_id(
        &database,
        &row.id,
        UpdateKeywordPrimaryKeyInput {
            value: Some("updated".into()),
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(updated.value, "updated");
    assert!(
        KeywordPrimaryKey::delete_by_id(&database, &row.id)
            .await
            .unwrap()
    );
    assert!(
        KeywordPrimaryKey::find_by_id(&database, &row.id)
            .await
            .unwrap()
            .is_none()
    );

    let schema = schema_builder(database).finish();
    let created = schema
        .execute("mutation { createKeywordPrimaryKey(input: { value: \"graphql\" }) { success keywordPrimaryKey { id value } } }")
        .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let created = created.data.into_json().unwrap();
    assert_eq!(created["createKeywordPrimaryKey"]["success"], true);
    let id = created["createKeywordPrimaryKey"]["keywordPrimaryKey"]["id"]
        .as_str()
        .unwrap();
    let updated = schema
        .execute(format!(
            "mutation {{ updateKeywordPrimaryKey(id: \"{id}\", input: {{ value: \"graphql-updated\" }}) {{ success keywordPrimaryKey {{ id value }} }} }}"
        ))
        .await;
    assert!(updated.errors.is_empty(), "{:?}", updated.errors);
    let updated = updated.data.into_json().unwrap();
    assert_eq!(updated["updateKeywordPrimaryKey"]["success"], true);
    assert_eq!(
        updated["updateKeywordPrimaryKey"]["keywordPrimaryKey"]["value"],
        "graphql-updated"
    );
    let deleted = schema
        .execute(format!(
            "mutation {{ deleteKeywordPrimaryKey(id: \"{id}\") {{ success keywordPrimaryKey {{ id }} }} }}"
        ))
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
    assert_eq!(
        deleted.data.into_json().unwrap()["deleteKeywordPrimaryKey"]["success"],
        true
    );
}

fn cid(value: &str) -> CollectionId {
    CollectionId::new(value).unwrap()
}

fn fid(value: &str) -> FieldId {
    FieldId::new(value).unwrap()
}

fn field(name: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
    RuntimeField {
        description: None,
        classification: None,
        export: None,
        id: fid(&format!("order.{name}")),
        api_name: name.into(),
        physical_column: name.into(),
        value_kind: kind,
        nullable,
        unique: false,
        filterable: true,
        sortable: true,
        generated: false,
        default: None,
        format: None,
        auto_timestamp: None,
        auto_actor: None,
    }
}

/// A runtime collection with a reserved table name, reserved columns and a
/// reserved index name. `extra` adds a reserved nullable column; `required`
/// makes `select` NOT NULL, which SQLite applies with a table rebuild.
fn runtime_definition(extra: bool, required: bool) -> RuntimeSchema {
    let mut fields = vec![
        field("id", RuntimeValueKind::Integer, false),
        field("join", RuntimeValueKind::String, false),
        field("select", RuntimeValueKind::String, !required),
    ];
    if extra {
        fields.push(field("where", RuntimeValueKind::String, true));
    }
    RuntimeSchema {
        format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
        collections: vec![RuntimeCollection {
            description: None,
            classification: None,
            id: cid("order"),
            api_type_name: "Order".into(),
            api_plural_name: "Orders".into(),
            physical_table: "order".into(),
            primary_key: vec![fid("order.id")],
            append_only: false,
            retention_purge: false,
            fields,
            relations: vec![],
            indexes: vec![RuntimeIndex {
                id: IndexId::new("order.group").unwrap(),
                name: "group".into(),
                fields: vec![fid("order.join")],
                unique: false,
            }],
            composite_unique: vec![],
            default_order: vec![],
            label_fields: Vec::new(),
            search: None,
        }],
    }
}

/// Plan, apply, prove the re-plan is a no-op and return the certified environment.
async fn migrate(
    database: &Database<Backend>,
    definition: RuntimeSchema,
    label: &str,
) -> (Vec<String>, RuntimeMutationEnvironment) {
    let schema = Arc::new(definition.validate().unwrap());
    let target = schema
        .physical_schema::<Backend>(Default::default())
        .unwrap();
    let ownership = ManagedTableSet::new(["order".to_string()]).unwrap();
    let plan = database
        .schema()
        .plan_owned_migration(
            label,
            "reserved names",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap();
    let statements = plan.statements().to_vec();
    database
        .schema()
        .apply_owned_migration(
            &plan,
            ApplyOptions {
                allow_destructive: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let noop = database
        .schema()
        .plan_owned_migration(
            &format!("{label}-noop"),
            "unchanged",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap();
    assert!(noop.steps().is_empty(), "{:?}", noop.statements());
    let environment = database
        .schema()
        .runtime_mutation_environment(schema, &target, &ownership)
        .await
        .unwrap();
    (statements, environment)
}

/// Approves every write of this isolated fixture.
#[derive(Clone)]
struct Authority(Arc<ValidatedRuntimeSchema>);

impl RuntimeWriteAuthority<Backend> for Authority {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move {
            let handle = check.collection();
            let fields = self.0.schema().collections[0]
                .fields
                .iter()
                .map(|field| self.0.resolve_field(handle, &field.id).unwrap())
                .collect::<Vec<_>>();
            RuntimeWriteGrant::new(self.0.resolve_projection(handle, &fields).unwrap(), None)
        })
    }
    fn authorize_preimage<'a>(
        &'a self,
        _: RuntimePreimageCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async { Ok(()) })
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        Box::pin(async move { Ok(RuntimeReturnGrant::new(check.intent.returning().cloned())) })
    }
    fn link_target_visibility<'a>(
        &'a self,
        _: RuntimeLinkVisibilityCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<Option<RuntimePredicate>, RuntimeMutationError>> {
        Box::pin(async { Ok(None) })
    }
    fn authorize_links<'a>(
        &'a self,
        _: RuntimeLinkCheck<'a>,
        _: &'a mut MutationContext<'_, Backend>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>> {
        Box::pin(async { Ok(()) })
    }
}

async fn mutate(
    database: &Database<Backend>,
    environment: &Arc<RuntimeMutationEnvironment>,
    request: RuntimeMutationRequest,
) -> RuntimeMutationEffect {
    let environment = environment.clone();
    let authority = Authority(environment.schema().clone());
    database
        .transaction(TransactionMode::StateMachine, move |tx| {
            Box::pin(async move {
                tx.mutate_runtime(&environment, &request, &authority)
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
        .unwrap()
}

/// The `select` values of rows whose `join` is `inner`, ordered by `select` descending.
async fn inner_selects(
    database: &Database<Backend>,
    schema: &ValidatedRuntimeSchema,
) -> Vec<Option<String>> {
    let limits = RuntimeQueryLimits::default();
    let collection = schema.resolve_collection(&cid("order")).unwrap();
    let join = schema
        .resolve_field(&collection, &fid("order.join"))
        .unwrap();
    let select = schema
        .resolve_field(&collection, &fid("order.select"))
        .unwrap();
    let filter = schema
        .runtime_compare(
            &collection,
            &join,
            RuntimeScalarOperator::Eq,
            RuntimeValue::String("inner".into()),
            limits,
        )
        .unwrap();
    let order = schema
        .runtime_order(
            &collection,
            Some(vec![RuntimeOrderInput {
                field: select.clone(),
                direction: RuntimeOrderDirection::Desc,
                nulls: RuntimeNullPlacement::Last,
            }]),
            limits,
        )
        .unwrap();
    let projection = schema
        .resolve_projection(&collection, std::slice::from_ref(&select))
        .unwrap();
    let request = schema
        .runtime_read_request(
            &collection,
            &projection,
            Some(filter),
            order,
            RuntimePageRequest::first(10, None),
            true,
            limits,
        )
        .unwrap();
    let page = database.execute_runtime_read(&request, None).await.unwrap();
    assert_eq!(page.total_count, Some(page.edges.len() as i64));
    page.edges
        .iter()
        .map(|edge| match edge.node.value(&select).unwrap() {
            RuntimeValue::String(value) => Some(value.clone()),
            RuntimeValue::Null => None,
            other => panic!("unexpected select value {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn runtime_collections_with_reserved_names_migrate_rebuild_and_serve_crud() {
    let database = database("keyword-identifiers-runtime").await;
    let (statements, environment) =
        migrate(&database, runtime_definition(false, false), "v1").await;
    assert!(
        statements
            .iter()
            .any(|sql| sql.starts_with("CREATE TABLE \"order\" ")),
        "{statements:?}"
    );
    let environment = Arc::new(environment);
    let schema = environment.schema().clone();
    let collection = schema.resolve_collection(&cid("order")).unwrap();
    let handle = |name: &str| {
        schema
            .resolve_field(&collection, &fid(&format!("order.{name}")))
            .unwrap()
    };
    for (id, join, select) in [(1, "inner", "a"), (2, "inner", "b"), (3, "outer", "c")] {
        let request = schema
            .runtime_create_request(
                &collection,
                &[
                    (handle("id"), RuntimeValue::Integer(id)),
                    (handle("join"), RuntimeValue::String(join.into())),
                    (handle("select"), RuntimeValue::String(select.into())),
                ],
                None,
                Default::default(),
            )
            .unwrap();
        mutate(&database, &environment, request).await;
    }
    assert_eq!(
        inner_selects(&database, &schema).await,
        [Some("b".into()), Some("a".into())]
    );

    let key = |id: i64| {
        schema
            .runtime_key(&collection, &[(handle("id"), RuntimeValue::Integer(id))])
            .unwrap()
    };
    let update = schema
        .runtime_update_request(
            key(1),
            &[(handle("select"), RuntimeValue::String("z".into()))],
            None,
            None,
            Default::default(),
        )
        .unwrap();
    mutate(&database, &environment, update).await;
    let delete = schema
        .runtime_delete_request(key(2), None, None, Default::default())
        .unwrap();
    mutate(&database, &environment, delete).await;
    assert_eq!(inner_selects(&database, &schema).await, [Some("z".into())]);

    // A reserved column is added in place on both backends.
    let (statements, _) = migrate(&database, runtime_definition(true, false), "v2").await;
    assert!(
        statements
            .iter()
            .any(|sql| sql.starts_with("ALTER TABLE \"order\" ADD COLUMN \"where\" ")),
        "{statements:?}"
    );

    // Tightening nullability rebuilds the table on SQLite and alters it in place
    // on PostgreSQL; rows survive either way.
    let (statements, environment) = migrate(&database, runtime_definition(true, true), "v3").await;
    #[cfg(feature = "sqlite")]
    assert!(
        statements.iter().any(|sql| sql
            == "INSERT INTO __graphql_orm_order_new (id, \"join\", \"select\", \"where\") \
                SELECT id, \"join\", \"select\", \"where\" FROM \"order\""),
        "{statements:?}"
    );
    #[cfg(feature = "postgres")]
    assert!(
        statements
            .iter()
            .any(|sql| sql == "ALTER TABLE \"order\" ALTER COLUMN \"select\" SET NOT NULL"),
        "{statements:?}"
    );
    assert_eq!(
        inner_selects(&database, environment.schema()).await,
        [Some("z".into())]
    );
}
