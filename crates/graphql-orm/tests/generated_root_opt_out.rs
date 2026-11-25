#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::prelude::*;

const WRITE_ROOTS: [&str; 5] = [
    "createWriteRecord",
    "deleteWriteRecord",
    "deleteWriteRecords",
    "updateWriteRecord",
    "updateWriteRecords",
];

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "quiet_records",
    auth = "none",
    mutations = false,
    subscriptions = false
)]
pub struct QuietRecord {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    title: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "watched_records",
    auth = "none",
    mutations = false,
    changed_started = true
)]
pub struct WatchedRecord {
    #[primary_key]
    #[sortable]
    id: String,
    title: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "write_records",
    auth = "none",
    subscriptions = false,
    ai_mutations(create = "automatic")
)]
pub struct WriteRecord {
    #[primary_key]
    #[sortable]
    id: String,
    title: String,
}

mod legacy_root_types {
    use super::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(table = "default_records", auth = "none")]
    struct DefaultRecord {
        #[primary_key]
        #[sortable]
        id: String,
        title: String,
    }

    schema_roots! { entities: [DefaultRecord] }

    #[tokio::test]
    async fn default_schema_keeps_the_original_tuple_roots() {
        let MutationRoot(mutations) = MutationRoot(MutationRootChunk0(DefaultRecordMutations));
        let SubscriptionRoot(subscriptions) =
            SubscriptionRoot(SubscriptionRootChunk0(DefaultRecordSubscriptions));
        let schema: AppSchema = graphql_orm::async_graphql::Schema::build(
            QueryRoot::default(),
            MutationRoot(mutations),
            SubscriptionRoot(subscriptions),
        )
        .finish();

        let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        let builder: graphql_orm::async_graphql::SchemaBuilder<
            QueryRoot,
            MutationRoot,
            SubscriptionRoot,
        > = schema_builder(db);
        assert_eq!(schema.sdl(), builder.finish().sdl());
        assert!(schema.sdl().contains("createDefaultRecord("));
        assert!(schema.sdl().contains("defaultRecordChanged("));
    }
}

mod quiet {
    use super::*;
    schema_roots! { entities: [QuietRecord], client_metadata: true }
}

mod mixed {
    use super::*;
    schema_roots! { entities: [QuietRecord, WatchedRecord, WriteRecord], client_metadata: true }
}

#[tokio::test]
async fn omitting_every_write_and_stream_root_preserves_repository_writes_and_queries() {
    let database = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    graphql_orm::sqlx::query(
        "CREATE TABLE quiet_records (id TEXT PRIMARY KEY, title TEXT NOT NULL)",
    )
    .execute(database.pool())
    .await
    .unwrap();
    database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .insert::<QuietRecord>(CreateQuietRecordInput {
                        id: "b".into(),
                        title: "From transaction".into(),
                    })
                    .await?)
            })
        })
        .await
        .unwrap();
    QuietRecord::insert(
        &database,
        CreateQuietRecordInput {
            id: "a".into(),
            title: "Available".into(),
        },
    )
    .await
    .unwrap();

    let schema: graphql_orm::async_graphql::Schema<
        quiet::QueryRoot,
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    > = quiet::schema_builder(database).finish();
    let sdl = schema.sdl();
    assert!(!sdl.contains("mutation:"), "{sdl}");
    assert!(!sdl.contains("subscription:"), "{sdl}");
    assert!(!sdl.contains("CreateQuietRecordInput"), "{sdl}");
    assert!(!sdl.contains("QuietRecordChangedEvent"), "{sdl}");
    let response = schema
        .execute(r#"{ a: quietRecord(id: "a") { title } b: quietRecord(id: "b") { title } }"#)
        .await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap(),
        serde_json::json!({
            "a": {"title": "Available"},
            "b": {"title": "From transaction"}
        }),
    );
    assert!(
        <QuietRecord as GraphqlOperationMetadata>::generated_graphql_operations()
            .iter()
            .all(|operation| operation.kind() == GraphqlOperationKind::Query)
    );
    assert!(quiet::graphql_orm_client_metadata().entities.is_empty());
}

#[tokio::test]
async fn independently_omitted_roots_are_neutral_in_composition() {
    use graphql_orm::futures::StreamExt;
    let database = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let schema = mixed::schema_builder(database).finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("createWriteRecord("), "{sdl}");
    assert!(!sdl.contains("createWatchedRecord("), "{sdl}");
    assert!(!sdl.contains("createQuietRecord("), "{sdl}");
    assert!(sdl.contains("watchedRecordChanged("), "{sdl}");
    assert!(!sdl.contains("quietRecordChanged("), "{sdl}");
    assert!(!sdl.contains("writeRecordChanged("), "{sdl}");
    let mut stream =
        schema.execute_stream("subscription { watchedRecordChanged(started: true) { started } }");
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .expect("started frame");
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap(),
        serde_json::json!({"watchedRecordChanged": {"started": true}}),
    );
}

#[tokio::test]
async fn catalogue_and_ai_output_follow_generated_root_exposure() {
    use graphql_orm_ai_tool_profiles::{
        AiGraphqlMutationCapabilityCatalog, AiGraphqlQueryCapabilityCatalog,
        AiGraphqlSubscriptionCapabilityCatalog, GraphqlExecutionTargetId,
    };

    let database = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let schema = mixed::schema_builder(database).finish();
    let sdl = schema.sdl();
    let catalog = mixed::graphql_orm_operation_catalog();
    let semantic = mixed::graphql_orm_semantic_catalog();
    let generated_roots = catalog
        .exposed_operations()
        .map(|op| (op.kind(), op.field_name()));
    let semantic_roots = semantic
        .operations
        .iter()
        .map(|op| (op.kind, op.field_name.as_str()));
    for roots in [
        generated_roots.collect::<Vec<_>>(),
        semantic_roots.collect::<Vec<_>>(),
    ] {
        assert!(roots.contains(&(GraphqlOperationKind::Query, "quietRecord")));
        let fields_for = |root_kind| {
            roots
                .iter()
                .filter(|(kind, _)| *kind == root_kind)
                .map(|(_, field)| *field)
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(
            fields_for(GraphqlOperationKind::Mutation),
            WRITE_ROOTS.into()
        );
        assert_eq!(
            fields_for(GraphqlOperationKind::Subscription),
            ["watchedRecordChanged"].into()
        );
    }

    let client = mixed::graphql_orm_client_metadata();
    client.validate_against(semantic).unwrap();
    let streams: Vec<_> = client
        .entities
        .iter()
        .filter(|entity| entity.changed_stream.is_some())
        .map(|entity| entity.entity_name.as_str())
        .collect();
    assert_eq!(streams, ["WatchedRecord"]);

    let target = GraphqlExecutionTargetId::parse("records").unwrap();
    let queries = AiGraphqlQueryCapabilityCatalog::compile(
        "records",
        target.clone(),
        &sdl,
        semantic,
        Default::default(),
    )
    .unwrap();
    assert!(
        queries
            .capabilities()
            .any(|query| query.field_name() == "quietRecord")
    );
    let mutations = AiGraphqlMutationCapabilityCatalog::compile(
        "records",
        target.clone(),
        &sdl,
        semantic,
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        mutations
            .capabilities()
            .map(|mutation| mutation.field_name())
            .collect::<Vec<_>>(),
        ["createWriteRecord"]
    );
    let subscriptions = AiGraphqlSubscriptionCapabilityCatalog::compile(
        "records",
        target,
        &sdl,
        semantic,
        Default::default(),
    )
    .unwrap();
    // The remaining Changed subscription offers best-effort delivery.
    assert_eq!(subscriptions.capabilities().count(), 0);
}

#[cfg(feature = "router-protocol")]
#[test]
fn router_exports_only_retained_root_fields() {
    use graphql_orm_router_protocol::RootOperationType;

    let operations = mixed::graphql_orm_operation_catalog()
        .router_protocol_operations()
        .unwrap();
    let fields_for = |root_type| {
        operations
            .iter()
            .filter(move |op| op.root_type == root_type)
            .map(|op| op.field_name.as_str())
            .collect::<std::collections::BTreeSet<_>>()
    };

    assert!(fields_for(RootOperationType::Query).contains("quietRecord"));
    assert_eq!(fields_for(RootOperationType::Mutation), WRITE_ROOTS.into());
    assert_eq!(
        fields_for(RootOperationType::Subscription),
        ["watchedRecordChanged"].into()
    );
}

mod propagation {
    use super::*;
    use graphql_orm::futures::StreamExt;
    use graphql_orm::graphql::orm::{ChangeAction, ChangeKind};
    use std::time::Duration;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        table = "silent_nodes",
        auth = "none",
        mutations = false,
        subscriptions = false
    )]
    struct SilentNode {
        #[primary_key]
        #[sortable]
        #[graphql_orm(auto_generated = false)]
        id: String,
        #[filterable(type = "string")]
        watched_id: Option<String>,
        #[graphql(skip)]
        #[relation(
            target = "WatchedNode",
            from = "watched_id",
            to = "id",
            propagate_change = "up"
        )]
        watched: Option<Box<WatchedNode>>,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(table = "watched_nodes", auth = "none", changed_started = true)]
    struct WatchedNode {
        #[primary_key]
        #[sortable]
        #[graphql_orm(auto_generated = false)]
        id: String,
        #[filterable(type = "string")]
        silent_id: Option<String>,
        #[graphql(skip)]
        #[relation(
            target = "SilentNode",
            from = "silent_id",
            to = "id",
            propagate_change = "up"
        )]
        silent: Option<Box<SilentNode>>,
    }

    schema_roots! { entities: [SilentNode, WatchedNode] }

    #[tokio::test]
    async fn propagation_works_in_both_directions_across_an_opt_out() {
        let database = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE silent_nodes (id TEXT PRIMARY KEY, watched_id TEXT);
             CREATE TABLE watched_nodes (id TEXT PRIMARY KEY, silent_id TEXT);
             INSERT INTO silent_nodes (id) VALUES ('silent-parent');
             INSERT INTO watched_nodes (id) VALUES ('watched-parent');",
        )
        .execute(database.pool())
        .await
        .unwrap();

        let schema = schema_builder(database.clone()).finish();
        let mut watched = schema.execute_stream(
            r#"subscription { watchedNodeChanged(key: "watched-parent", started: true) {
                started action changeKind sourceEntity sourceId path
            } }"#,
        );
        let wait = Duration::from_secs(5);
        let started = tokio::time::timeout(wait, watched.next())
            .await
            .unwrap()
            .unwrap();
        assert!(started.errors.is_empty(), "{started:?}");
        assert_eq!(
            started.data.into_json().unwrap()["watchedNodeChanged"]["started"],
            true
        );

        SilentNode::insert(
            &database,
            CreateSilentNodeInput {
                id: "silent-child".into(),
                watched_id: Some("watched-parent".into()),
            },
        )
        .await
        .unwrap();

        let propagated = tokio::time::timeout(wait, watched.next())
            .await
            .unwrap()
            .unwrap();
        assert!(propagated.errors.is_empty(), "{propagated:?}");
        let propagated = propagated.data.into_json().unwrap();
        assert_eq!(
            propagated["watchedNodeChanged"],
            serde_json::json!({
                "started": false, "action": "CREATED", "changeKind": "PROPAGATED",
                "sourceEntity": "SilentNode", "sourceId": "silent-child", "path": ["watched"],
            })
        );

        // The opted-out parent has no GraphQL Changed field, but still receives
        // propagated events through the host event stream.
        let mut silent = database
            .ensure_event_sender::<SilentNodeChangedEvent>()
            .subscribe();
        WatchedNode::insert(
            &database,
            CreateWatchedNodeInput {
                id: "watched-child".into(),
                silent_id: Some("silent-parent".into()),
            },
        )
        .await
        .unwrap();

        let propagated = tokio::time::timeout(wait, silent.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(propagated.action, ChangeAction::Created);
        assert_eq!(propagated.change_kind, ChangeKind::Propagated);
        assert_eq!(propagated.id, "silent-parent");
        assert_eq!(propagated.source_entity.as_deref(), Some("WatchedNode"));
        assert_eq!(propagated.source_id.as_deref(), Some("watched-child"));
        assert_eq!(propagated.path, ["silent"]);
    }
}
