//! Current public APIs only; source-prepared, not compiled yet.
use graphql_orm::graphql::orm::*;
#[cfg(feature="sqlite")]
pub type ProbeBackend = SqliteBackend;
#[cfg(feature="postgres")]
pub type ProbeBackend = PostgresBackend;
#[cfg(all(feature="sqlite",feature="postgres"))]
compile_error!("select one probe backend");

pub fn schema() -> ValidatedRuntimeSchema {
    let field = |name: &str, filterable| RuntimeField {
        id: FieldId::new(name).unwrap(), api_name: name.into(), physical_column: name.into(),
        value_kind: RuntimeValueKind::Integer, nullable: false, unique: name == "id",
        filterable, sortable: false, generated: false, default: None,
    };
    RuntimeSchema { format_version: 1, collections: vec![RuntimeCollection {
        id: CollectionId::new("records").unwrap(), api_type_name: "Record".into(),
        api_plural_name: "Records".into(), physical_table: "records".into(),
        primary_key: vec![FieldId::new("id").unwrap()], append_only: false,
        retention_purge: false, fields: vec![field("id", false), field("tenant", true)],
        relations: vec![], indexes: vec![], composite_unique: vec![], default_order: vec![],
    }] }.validate().unwrap()
}

#[cfg(feature="gap-key-read")]
pub fn key_is_not_a_read_predicate(schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle, projection: &RuntimeProjection,
    key: RuntimeKey, order: RuntimeOrder, page: RuntimePageRequest) {
    // Expected E0308: the existing point-key handle cannot enter the public
    // read request's predicate slot. This alone does not prove absence of every API.
    let _ = schema.runtime_read_request(collection, projection, Some(key), order,
        page, false, RuntimeQueryLimits::default());
}
#[cfg(feature="gap-record-match")]
pub fn no_public_predicate_evaluator(predicate: &RuntimePredicate, record: &RuntimeRecord) {
    // Expected E0599; source audit separately checks other public matching APIs.
    let _ = predicate.matches_record(record);
}
#[cfg(feature="gap-lowering")]
pub fn canonical_lowering_is_private() {
    // Expected E0603: canonical lowering module is not a public consumer surface.
    use graphql_orm::graphql::runtime::plan::filter;
    let _ = filter;
}
#[cfg(feature="gap-static-record")]
pub async fn static_reader_needs_compiled_entity(
    db: &graphql_orm::db::Database<ProbeBackend>, ctx: &graphql_orm::async_graphql::Context<'_>) {
    // Expected trait-bound errors: owned RuntimeRecord is not a compiled Entity.
    let _ = graphql_orm::graphql::subscriptions::authorize_subscription_row::<
        RuntimeRecord, ProbeBackend>(db, ctx, &[], None).await;
}
