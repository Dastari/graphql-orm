#![cfg(feature = "sqlite")]

use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(RepositoryEntity, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "private_parents", plural = "PrivateParents")]
struct PrivateParent {
    #[primary_key]
    #[graphql_orm(auto_generated = false, sensitive)]
    id: String,
    #[graphql_orm(sensitive)]
    secret: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(table = "public_children", plural = "PublicChildren")]
struct PublicChild {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    parent_id: String,
    #[graphql(skip)]
    #[relation(
        target = "PrivateParent",
        from = "parent_id",
        to = "id",
        propagate_change = "up",
        emit_fk = false
    )]
    parent: Option<PrivateParent>,
}

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(table = "public_parents", plural = "PublicParents")]
struct PublicParent {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    label: String,
}

#[derive(RepositoryEntity, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "private_children", plural = "PrivateChildren")]
struct PrivateChild {
    #[primary_key]
    #[graphql_orm(auto_generated = false, sensitive)]
    id: String,
    parent_id: String,
    #[graphql_orm(sensitive)]
    secret: String,
    #[relation(
        target = "PublicParent",
        from = "parent_id",
        to = "id",
        propagate_change = "up",
        emit_fk = false
    )]
    parent: Option<PublicParent>,
}

schema_roots! { query_custom_ops: [], entities: [PublicParent, PublicChild], }

async fn fixture() -> Database {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    for sql in [
        "CREATE TABLE private_parents (id TEXT PRIMARY KEY, secret TEXT NOT NULL)",
        "CREATE TABLE public_children (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL)",
        "CREATE TABLE public_parents (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
        "CREATE TABLE private_children (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL, secret TEXT NOT NULL)",
    ] {
        sqlx::query(sql).execute(db.pool()).await.unwrap();
    }
    db
}

async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected initial event {other:?}"),
    })
    .await;
}

#[tokio::test]
async fn graphql_source_preserves_repository_target_redaction_without_witnesses() {
    let db = fixture().await;
    PrivateParent::insert(
        &db,
        CreatePrivateParentInput {
            id: "private-parent-key".into(),
            secret: "private-parent-secret".into(),
        },
    )
    .await
    .unwrap();
    let mut raw = db
        .ensure_event_sender::<PrivateParentChangedEvent>()
        .subscribe();
    let mut witnesses = db
        .ensure_event_sender::<graphql_orm::graphql::subscriptions::SubscriptionEvent<
            PrivateParentChangedEvent,
            SqliteBackend,
        >>()
        .subscribe();
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("alice"))
        .finish();
    let result = schema.execute("mutation { createPublicChild(input: {id: \"public-child\", parentId: \"private-parent-key\"}) { success error } }").await;
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        result.data.into_json().unwrap()["createPublicChild"]["success"],
        true
    );
    let event = timeout(Duration::from_secs(2), raw.recv())
        .await
        .unwrap()
        .unwrap();
    // A closed pattern proves sensitive identity and payload fields do not exist.
    let PrivateParentChangedEvent {
        action: _,
        change_kind,
        source_entity,
        source_id,
        path,
    } = event;
    assert_eq!(change_kind, ChangeKind::Propagated);
    let rendered = format!("{source_entity:?} {source_id:?} {path:?}");
    assert!(!rendered.contains("private-parent"));
    assert!(matches!(
        witnesses.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn repository_source_provenance_cannot_enter_public_graphql_delivery() {
    let db = fixture().await;
    PublicParent::insert(
        &db,
        CreatePublicParentInput {
            id: "public-parent".into(),
            label: "before".into(),
        },
    )
    .await
    .unwrap();
    let mut raw = db
        .ensure_event_sender::<PrivateChildChangedEvent>()
        .subscribe();
    let mut witnesses = db
        .ensure_event_sender::<graphql_orm::graphql::subscriptions::SubscriptionEvent<
            PrivateChildChangedEvent,
            SqliteBackend,
        >>()
        .subscribe();
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("alice"))
        .finish();
    let mut stream = Box::pin(schema.execute_stream(Request::new("subscription { publicParentChanged { id changeKind sourceId path publicParent { label } } }")));
    registered(&mut stream).await;
    PrivateChild::insert(
        &db,
        CreatePrivateChildInput {
            id: "private-child-key".into(),
            parent_id: "public-parent".into(),
            secret: "private-child-secret".into(),
        },
    )
    .await
    .unwrap();
    let event = timeout(Duration::from_secs(2), raw.recv())
        .await
        .unwrap()
        .unwrap();
    let PrivateChildChangedEvent {
        action: _,
        change_kind,
        source_entity,
        source_id,
        path,
    } = event;
    assert_eq!(change_kind, ChangeKind::Direct);
    let rendered = format!("{source_entity:?} {source_id:?} {path:?}");
    assert!(!rendered.contains("private-child"));
    assert!(matches!(
        witnesses.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    // A public direct change follows the denied propagated event in the same FIFO.
    let result = schema.execute("mutation { updatePublicParent(id: \"public-parent\", input: {label: \"authorized-control\"}) { success error } }").await;
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        result.data.into_json().unwrap()["updatePublicParent"]["success"],
        true
    );
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json().unwrap();
    assert_eq!(json["publicParentChanged"]["changeKind"], "DIRECT");
    assert_eq!(
        json["publicParentChanged"]["sourceId"],
        serde_json::Value::Null
    );
    assert_eq!(
        json["publicParentChanged"]["publicParent"]["label"],
        "authorized-control"
    );
    assert!(!json.to_string().contains("private-child"));
    assert!(
        <PrivateChild as Entity>::subscription_field_policy("id")
            .is_some_and(|(_, allowed, _)| !allowed)
    );
}
