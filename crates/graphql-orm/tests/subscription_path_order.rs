//! Alternate provenance paths remain independently authorized in either declaration order.
#![cfg(feature = "sqlite")]
use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use std::task::Poll;
use tokio::time::{Duration, timeout};
#[path = "support/graphql_case.rs"]
mod graphql_case;
use graphql_case::{argument_name, field_name, resolver_name};
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "path_targets", plural = "PathTargets")]
struct PathTarget {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    label: String,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "path_bridges", plural = "PathBridges")]
struct PathBridge {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    target_id: String,
    #[graphql(skip)]
    #[relation(
        target = "PathTarget",
        from = "target_id",
        to = "id",
        propagate_change = "up"
    )]
    target: Option<PathTarget>,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "direct_first", plural = "DirectFirsts")]
struct DirectFirst {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    target_id: String,
    bridge_id: String,
    #[graphql(skip)]
    #[relation(
        target = "PathTarget",
        from = "target_id",
        to = "id",
        propagate_change = "up"
    )]
    direct: Option<PathTarget>,
    #[graphql(skip)]
    #[relation(
        target = "PathBridge",
        from = "bridge_id",
        to = "id",
        propagate_change = "up"
    )]
    bridge: Option<PathBridge>,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "indirect_first", plural = "IndirectFirsts")]
struct IndirectFirst {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    target_id: String,
    bridge_id: String,
    #[graphql(skip)]
    #[relation(
        target = "PathBridge",
        from = "bridge_id",
        to = "id",
        propagate_change = "up"
    )]
    bridge: Option<PathBridge>,
    #[graphql(skip)]
    #[relation(
        target = "PathTarget",
        from = "target_id",
        to = "id",
        propagate_change = "up"
    )]
    direct: Option<PathTarget>,
}
schema_roots! { entities: [PathTarget, PathBridge, DirectFirst, IndirectFirst] }
#[derive(Clone)]
struct Policy;
impl EntityPolicy for Policy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl FieldPolicy for Policy {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let subject = ctx.auth_subject_opt().map(|s| s.id).unwrap_or_default();
            if entity == "DirectFirst" || entity == "IndirectFirst" {
                if field == field_name("direct") {
                    return Ok(subject == "direct");
                }
                if field == field_name("bridge") {
                    return Ok(subject == "indirect");
                }
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
async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected initial item {other:?}"),
    })
    .await;
}
#[tokio::test]
async fn both_orders_deliver_only_the_authorized_path() {
    for (mutation, payload) in [
        ("createDirectFirst", "directFirst"),
        ("createIndirectFirst", "indirectFirst"),
    ] {
        let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap();
        for sql in [
            "CREATE TABLE path_targets (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
            "CREATE TABLE path_bridges (id TEXT PRIMARY KEY, target_id TEXT NOT NULL)",
            "CREATE TABLE direct_first (id TEXT PRIMARY KEY, target_id TEXT NOT NULL, bridge_id TEXT NOT NULL)",
            "CREATE TABLE indirect_first (id TEXT PRIMARY KEY, target_id TEXT NOT NULL, bridge_id TEXT NOT NULL)",
            "INSERT INTO path_targets VALUES ('target', 'public')",
            "INSERT INTO path_bridges VALUES ('bridge', 'target')",
        ] {
            sqlx::query(sql).execute(db.pool()).await.unwrap();
        }
        db.set_entity_policy(Policy);
        db.set_field_policy(Policy);
        let schema = schema_builder(db).finish();
        let subscription = format!(
            "subscription {{ {} {{ {} {} }} }}",
            resolver_name("pathTargetChanged"),
            field_name("id"),
            field_name("path")
        );
        let mut direct = Box::pin(
            schema.execute_stream(Request::new(&subscription).data(AuthSubject::new("direct"))),
        );
        let mut indirect = Box::pin(
            schema.execute_stream(Request::new(&subscription).data(AuthSubject::new("indirect"))),
        );
        let mut neither = Box::pin(
            schema.execute_stream(Request::new(&subscription).data(AuthSubject::new("neither"))),
        );
        registered(&mut direct).await;
        registered(&mut indirect).await;
        registered(&mut neither).await;
        let response = schema.execute(Request::new(format!("mutation {{ {}({}: {{ {}: \"source\", {}: \"target\", {}: \"bridge\" }}) {{ {} {{ {} }} }} }}", resolver_name(mutation), argument_name("input"), field_name("id"), field_name("targetId"), field_name("bridgeId"), field_name(payload), field_name("id"))).data(AuthSubject::new("writer"))).await;
        assert!(response.errors.is_empty(), "{response:?}");
        for (stream, expected) in [
            (&mut direct, vec![field_name("direct")]),
            (
                &mut indirect,
                vec![field_name("bridge"), field_name("target")],
            ),
        ] {
            let response = timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap();
            assert!(response.errors.is_empty(), "{response:?}");
            let data = response.data.into_json().unwrap();
            assert_eq!(
                data[resolver_name("pathTargetChanged")][field_name("path")],
                serde_json::json!(expected)
            );
            assert!(
                timeout(Duration::from_millis(100), stream.next())
                    .await
                    .is_err()
            );
        }
        assert!(
            timeout(Duration::from_millis(100), neither.next())
                .await
                .is_err()
        );
    }
}
