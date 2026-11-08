//! Unreadable fields must not enter generated aggregate or catalogue surfaces.
#![cfg(feature = "sqlite")]
#![cfg(not(any(feature = "postgres", feature = "mssql")))]

use graphql_orm::async_graphql::{ComplexObject, SimpleObject};
use graphql_orm::prelude::*;

/// Writable secrets remain hidden across generated read surfaces.
#[derive(
    GraphQLEntity,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "probe_write_only",
    plural = "ProbeWriteOnlys",
    default_sort = "id ASC",
    aggregate = true,
    keyset = "salary asc, id asc",
    auth = "none"
)]
struct ProbeWriteOnly {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[sortable]
    label: String,
    #[input_only]
    #[graphql(skip)]
    secret: String,
    #[input_only]
    #[graphql(skip)]
    salary: i64,
}
#[ComplexObject]
impl ProbeWriteOnly {}

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    GraphQLRelations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "relation_records",
    plural = "RelationRecords",
    auth = "none"
)]
struct RelationRecord {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    label: String,
    #[graphql_orm(private)]
    #[graphql(skip)]
    target_id: String,
    #[input_only]
    #[graphql(skip)]
    #[relation(
        target = "ProbeWriteOnly",
        from = "target_id",
        to = "id",
        emit_fk = false
    )]
    hidden: Option<ProbeWriteOnly>,
}

#[ComplexObject]
impl RelationRecord {}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [ProbeWriteOnly, RelationRecord],
}

async fn database() -> Database<SqliteBackend> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    let plan = database
        .schema()
        .plan_migration_to_entities("probe-init", "probe schema", &[ProbeWriteOnly::metadata()])
        .await
        .unwrap();
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    database
}

#[tokio::test]
async fn input_only_fields_are_absent_from_aggregates_and_catalogues() {
    let db = database().await;
    graphql_orm::sqlx::query(
        "INSERT INTO probe_write_only (id, label, secret, salary) VALUES ('r1', 'row', 'hunter2', 987654)",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let schema = schema_builder(db).finish();
    let sdl = schema.sdl();
    let start = sdl.find("type ProbeWriteOnly ").expect("object type");
    let object = &sdl[start..start + sdl[start..].find('}').unwrap()];
    assert!(!object.contains("secret"), "object leaked: {object}");
    let enum_start = sdl
        .find("enum ProbeWriteOnlyAggregateField")
        .expect("aggregate enum");
    let aggregate_enum = &sdl[enum_start..enum_start + sdl[enum_start..].find('}').unwrap()];
    assert!(!aggregate_enum.contains("secret"));
    assert!(!aggregate_enum.contains("salary"));
    let metadata = <ProbeWriteOnly as Entity>::graphql_semantic_metadata().unwrap();
    assert!(
        !metadata
            .fields
            .iter()
            .any(|field| matches!(field.field_name.as_str(), "secret" | "salary"))
    );
    let query = "{ probeWriteOnlysAggregate(groupBy: [secret], metrics: [{operator: MAX, field: salary}], groupLimit: 10) { groups { field kind value } metrics { field operator kind value } } }";
    let response = schema.execute(query).await;
    let rendered = serde_json::to_string(&response).unwrap();
    assert!(!response.errors.is_empty());
    assert!(
        !rendered.contains("hunter2") && !rendered.contains("987654"),
        "input_only values read through aggregate: {rendered}"
    );
    let response = schema.execute("{ probeWriteOnlysAggregate(groupBy: [], metrics: [{operator: MAX, field: salary}], groupLimit: 10) { metrics { value } } }").await;
    assert!(!response.errors.is_empty());
    let response = schema.execute("{ probeWriteOnlysAggregate(groupBy: [], metrics: [{operator: COUNT, field: salary}], groupLimit: 10) { metrics { value } } }").await;
    assert!(!response.errors.is_empty());
    let response = schema
        .execute("{ probeWriteOnlysKeyset(page: {limit: 1}) { edges { cursor node { id } } } }")
        .await;
    assert!(
        !response.errors.is_empty(),
        "hidden order values must never enter cursors"
    );
    let rendered = serde_json::to_string(&response).unwrap();
    assert!(!rendered.contains("987654"));
    let response = schema.execute("{ probeWriteOnlysAggregate(groupBy: [label], metrics: [{operator: COUNT}], groupLimit: 10) { groups { value } } }").await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
}

#[tokio::test]
async fn unreadable_relation_is_absent_from_sdl_and_catalogue() {
    let schema = schema_builder(database().await).finish();
    let sdl = schema.sdl();
    let start = sdl.find("type RelationRecord ").unwrap();
    let object = &sdl[start..start + sdl[start..].find('}').unwrap()];
    assert!(!object.contains("hidden"));
    let metadata = <RelationRecord as Entity>::graphql_semantic_metadata().unwrap();
    assert!(
        !metadata
            .fields
            .iter()
            .any(|field| field.field_name == "hidden")
    );
}
