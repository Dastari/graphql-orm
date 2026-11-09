#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "composite_audit",
    plural = "CompositeAudits",
    auth = "none",
    append_only = true
)]
struct CompositeAudit {
    #[primary_key]
    #[sortable]
    id: i32,
    #[primary_key]
    #[graphql_orm(db_column = "sequence_number")]
    sequence: i32,
    payload: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [CompositeAudit],
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn composite_append_only_creates_use_the_complete_key() -> graphql_orm::Result<()> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "composite-append-init",
            "composite append-only",
            &[CompositeAudit::metadata()],
        )
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let schema = schema_builder(db.clone()).finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("createCompositeAudit("), "{sdl}");
    for forbidden in [
        "updateCompositeAudit",
        "deleteCompositeAudit",
        "upsertCompositeAudit",
        "createManyCompositeAudit",
    ] {
        assert!(!sdl.contains(forbidden), "{sdl}");
    }
    let catalog = graphql_orm_operation_catalog();
    let writes: Vec<_> = catalog
        .operations()
        .iter()
        .filter(|op| op.kind() == graphql_orm::graphql::orm::GraphqlOperationKind::Mutation)
        .collect();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].field_name(), "createCompositeAudit");
    let semantic = graphql_orm_semantic_catalog();
    let semantic_writes: Vec<_> = semantic
        .operations
        .iter()
        .filter(|op| op.kind == graphql_orm::graphql::orm::GraphqlOperationKind::Mutation)
        .map(|op| op.field_name.as_str())
        .collect();
    assert_eq!(semantic_writes, ["createCompositeAudit"]);
    let mut events = db.ensure_event_sender::<graphql_orm::graphql::subscriptions::SubscriptionEvent<CompositeAuditChangedEvent>>().subscribe();
    let mut event_ids = Vec::new();
    for sequence in [1, 2] {
        let response = schema.execute(format!(r#"mutation {{ createCompositeAudit(input: {{ id: 7, sequence: {sequence}, payload: "event-{sequence}" }}) {{ success compositeAudit {{ id sequence payload }} }} }}"#)).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(data["createCompositeAudit"]["success"], true, "{data}");
        assert_eq!(
            data["createCompositeAudit"]["compositeAudit"]["sequence"],
            sequence
        );
        let event = events.try_recv().expect("committed create event");
        assert_eq!(event.event.key.id, 7);
        assert_eq!(event.event.key.sequence, sequence);
        assert_eq!(event.keys.len(), 2);
        event_ids.push(event.event.id.clone());
    }
    assert_ne!(event_ids[0], event_ids[1]);
    let row = CompositeAudit::get_by_key(db.pool(), &CompositeAuditKey { id: 7, sequence: 2 })
        .await?
        .unwrap();
    assert_eq!(row.payload, "event-2");
    let duplicate = CompositeAudit::insert(
        &db,
        CreateCompositeAuditInput {
            id: 7,
            sequence: 2,
            payload: "duplicate".into(),
        },
    )
    .await;
    assert!(duplicate.is_err());
    Ok(())
}
