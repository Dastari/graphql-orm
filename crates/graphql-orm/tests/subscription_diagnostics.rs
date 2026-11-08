// Regression probes for diagnostics buffered by host authorization callbacks.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]
use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    table = "probe_records",
    plural = "ProbeRecords",
    default_sort = "id ASC"
)]
struct ProbeRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    owner: String,
    #[graphql_orm(read_policy = "label.read")]
    label: String,
}
#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(table = "probe_audits", plural = "ProbeAudits", append_only = true)]
struct ProbeAudit {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    owner: String,
    #[graphql_orm(read_policy = "label.read")]
    label: String,
}
schema_roots! { query_custom_ops: [], entities: [ProbeRecord, ProbeAudit], }

fn identity(row: &(dyn std::any::Any + Send + Sync)) -> (&str, &str) {
    if let Some(row) = row.downcast_ref::<ProbeRecord>() {
        (&row.id, &row.owner)
    } else {
        let row = row.downcast_ref::<ProbeAudit>().unwrap();
        (&row.id, &row.owner)
    }
}

const SHAPES: [(&str, &str, &str); 2] = [
    ("probeRecordChanged", "probeRecord", "createProbeRecord"),
    ("probeAuditChanged", "probeAudit", "createProbeAudit"),
];

#[derive(Clone, Default)]
struct DiagnosticPolicy {
    diag_calls: Arc<AtomicUsize>,
}
impl FieldPolicy for DiagnosticPolicy {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let id = record.map(|r| identity(r).0).unwrap_or_default();
            if field == "label" && id.starts_with("diag") {
                self.diag_calls.fetch_add(1, Ordering::SeqCst);
                ctx.add_error(async_graphql::ServerError::new(
                    format!("private-field-diag-{id}"),
                    None,
                ));
                return Ok(false);
            }
            Ok(true)
        })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl RowPolicy for DiagnosticPolicy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async { Ok(ReadVisibility::CallbackOnly) })
    }
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let (id, owner) = identity(row);
            if id.starts_with("rowdiag") {
                self.diag_calls.fetch_add(1, Ordering::SeqCst);
                if let Some(ctx) = ctx {
                    ctx.add_error(async_graphql::ServerError::new(
                        format!("private-row-diag-{id}"),
                        None,
                    ));
                }
                return Ok(false);
            }
            Ok(ctx
                .and_then(|ctx| ctx.auth_subject_opt())
                .is_some_and(|s| s.id == owner))
        })
    }
    fn can_read_deleted_preimage<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        policy: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        self.can_read_row(ctx, db, entity, policy, surface, row)
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

async fn setup(capacity: usize) -> (AppSchema, DiagnosticPolicy, Database) {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    if capacity > 0 {
        db.set_event_channel_capacity(capacity);
    }
    sqlx::query("CREATE TABLE probe_records (id TEXT PRIMARY KEY, owner TEXT NOT NULL, label TEXT NOT NULL)")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE probe_audits (id TEXT PRIMARY KEY, owner TEXT NOT NULL, label TEXT NOT NULL)",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let p = DiagnosticPolicy::default();
    db.set_row_policy(p.clone());
    db.set_field_policy(p.clone());
    (schema_builder(db.clone()).finish(), p, db)
}

async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected frame: {other:?}"),
    })
    .await;
}
async fn publish(schema: &AppSchema, mutation: &str, id: &str, owner: &str) {
    let response = schema.execute(Request::new(format!(
        "mutation {{ {mutation}(input: {{id: \"{id}\", owner: \"{owner}\", label: \"l\"}}) {{ success error }} }}"
    )).data(AuthSubject::new(owner))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()[mutation]["success"],
        true
    );
}

async fn visible_frame_after_denial(hidden: &str) {
    for (root, payload, mutation) in SHAPES {
        for keyed in [false, true] {
            let (schema, policy, _) = setup(0).await;
            let args = if keyed {
                format!("(keys: [\"{hidden}\", \"visible\"])")
            } else {
                String::new()
            };
            let mut stream = Box::pin(
                schema.execute_stream(
                    Request::new(format!(
                        "subscription {{ {root}{args} {{ id {payload} {{ label }} }} }}"
                    ))
                    .data(AuthSubject::new("alice")),
                ),
            );
            registered(&mut stream).await;
            publish(&schema, mutation, hidden, "alice").await;
            publish(&schema, mutation, "visible", "alice").await;
            let frame = timeout(Duration::from_secs(5), stream.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                policy.diag_calls.load(Ordering::SeqCst),
                1,
                "{root} keyed={keyed}"
            );
            assert!(
                frame.errors.is_empty(),
                "{root} keyed={keyed}: {:?}",
                frame.errors
            );
            assert_eq!(frame.data.into_json().unwrap()[root]["id"], "visible");
        }
    }
}

// P1/P2: a silent denial must discard diagnostics before the next visible item.
#[tokio::test]
async fn field_diagnostics_from_dropped_events_do_not_reach_visible_frames() {
    visible_frame_after_denial("diag-hidden").await;
}

#[tokio::test]
async fn row_diagnostics_from_dropped_events_do_not_reach_visible_frames() {
    visible_frame_after_denial("rowdiag-hidden").await;
}

// P3: diagnostic cleanup must also precede a terminal lag frame.
#[tokio::test]
async fn dropped_event_diagnostics_do_not_reach_lag_frames() {
    for (root, _, mutation) in SHAPES {
        let (schema, policy, _) = setup(2).await;
        let mut stream = Box::pin(
            schema.execute_stream(
                Request::new(format!("subscription {{ {root} {{ id }} }}"))
                    .data(AuthSubject::new("alice")),
            ),
        );
        registered(&mut stream).await;
        publish(&schema, mutation, "rowdiag-hidden", "bob").await;
        timeout(Duration::from_secs(5), async {
            while policy.diag_calls.load(Ordering::SeqCst) == 0 {
                registered(&mut stream).await;
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // Let the suppressed event's future finish before overflowing its receiver.
        for _ in 0..50 {
            registered(&mut stream).await;
            tokio::task::yield_now().await;
        }
        for i in 0..5 {
            publish(&schema, mutation, &format!("hidden-{i}"), "bob").await;
        }
        let frame = timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(frame.errors.len(), 1, "{root}: {:?}", frame.errors);
        assert_eq!(
            frame.errors[0].extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::String(
                "SUBSCRIPTION_CONTINUITY_LOST".into()
            ))
        );
        assert_eq!(frame.data, async_graphql::Value::Null);
        assert!(stream.next().await.is_none());
    }
}
