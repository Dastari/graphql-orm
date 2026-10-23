#![cfg(feature = "sqlite")]

use graphql_orm::async_graphql::Response;
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(table = "subscription_integer_records", plural = "IntegerRecords")]
struct IntegerRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "number")]
    #[sortable]
    id: i32,
    label: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    table = "subscription_integer_audits",
    plural = "IntegerAudits",
    append_only = true
)]
struct IntegerAudit {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "number")]
    #[sortable]
    id: i32,
    label: String,
}

schema_roots! {
    query_custom_ops: [],
    generated_mutations: "none",
    entities: [IntegerRecord, IntegerAudit],
}

async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected registration response: {other:?}"),
    })
    .await;
}

fn assert_identity(response: Response, root: &str, row: &str, expected: i32) {
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data[root]["id"], expected);
    assert_eq!(data[root][row]["id"], expected);
    assert_eq!(data[root]["action"], "CREATED");
}

#[tokio::test]
async fn normal_and_append_only_integer_keys_filter_exactly_without_changing_omission() {
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    for table in [
        "subscription_integer_records",
        "subscription_integer_audits",
    ] {
        sqlx::query(&format!(
            "CREATE TABLE {table} (id INTEGER PRIMARY KEY, label TEXT NOT NULL)"
        ))
        .execute(db.pool())
        .await
        .unwrap();
    }
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("integer-key-fixture"))
        .finish();
    let sdl = schema.sdl();
    for root in ["integerRecordChanged", "integerAuditChanged"] {
        let prefix = format!("{root}(");
        let arguments = sdl
            .split(prefix.as_str())
            .nth(1)
            .and_then(|suffix| suffix.split(')').next())
            .expect("generated subscription arguments");
        assert!(arguments.contains("key: Int"), "{root}: {arguments}");
        assert!(!arguments.contains("key: Int!"), "key must remain optional");
    }

    let mut records = Box::pin(schema.execute_stream(
        "subscription { integerRecordChanged { id action integerRecord { id label } } }",
    ));
    let mut exact_record = Box::pin(schema.execute_stream(
        "subscription { integerRecordChanged(key: 1) { id action integerRecord { id label } } }",
    ));
    let mut audits = Box::pin(schema.execute_stream(
        "subscription { integerAuditChanged { id action integerAudit { id label } } }",
    ));
    let mut exact_audit = Box::pin(schema.execute_stream(
        "subscription { integerAuditChanged(key: 1) { id action integerAudit { id label } } }",
    ));
    registered(&mut records).await;
    registered(&mut exact_record).await;
    registered(&mut audits).await;
    registered(&mut exact_audit).await;

    for id in [2, 1] {
        IntegerRecord::insert(
            &db,
            CreateIntegerRecordInput {
                id,
                label: format!("record-{id}"),
            },
        )
        .await
        .unwrap();
        IntegerAudit::insert(
            &db,
            CreateIntegerAuditInput {
                id,
                label: format!("audit-{id}"),
            },
        )
        .await
        .unwrap();
    }
    for expected in [2, 1] {
        let record = timeout(Duration::from_secs(2), records.next())
            .await
            .unwrap()
            .unwrap();
        assert_identity(record, "integerRecordChanged", "integerRecord", expected);
        let audit = timeout(Duration::from_secs(2), audits.next())
            .await
            .unwrap()
            .unwrap();
        assert_identity(audit, "integerAuditChanged", "integerAudit", expected);
    }
    let record = timeout(Duration::from_secs(2), exact_record.next())
        .await
        .unwrap()
        .unwrap();
    assert_identity(record, "integerRecordChanged", "integerRecord", 1);
    let audit = timeout(Duration::from_secs(2), exact_audit.next())
        .await
        .unwrap()
        .unwrap();
    assert_identity(audit, "integerAuditChanged", "integerAudit", 1);
    assert!(
        timeout(Duration::from_millis(100), exact_record.next())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(100), exact_audit.next())
            .await
            .is_err()
    );
}
