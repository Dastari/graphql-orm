#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use std::any::Any;
use std::sync::{Arc, Mutex};

use async_graphql::{Context, EmptyMutation, ErrorExtensions, PathSegment, Response, Schema};
use graphql_orm::futures::{Stream, StreamExt, future::BoxFuture};
use graphql_orm::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ScalarPayload {
    count: i32,
}

async_graphql::scalar!(ScalarPayload);

#[derive(GraphQLEntity, Clone, Debug, Serialize, Deserialize)]
#[graphql_entity(table = "dispatch_records", description = "Dispatch record.")]
#[graphql(rename_fields = "PascalCase")]
struct DispatchRecord {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql(name = "payload")]
    #[graphql_orm(json, description = "Payload.")]
    payload: ScalarPayload,
    #[graphql_orm(read_policy = "dispatch.optional")]
    optional_count: Option<i32>,
    #[graphql_orm(subscribe = false)]
    query_only: String,
    #[cfg(any())]
    omitted: String,
}

fn record() -> DispatchRecord {
    DispatchRecord {
        id: "a".into(),
        payload: ScalarPayload { count: 3 },
        optional_count: None,
        query_only: "query".into(),
    }
}

struct Query;

#[async_graphql::Object]
impl Query {
    async fn records(&self) -> Vec<DispatchRecord> {
        vec![record()]
    }

    async fn direct_getter(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<i32>> {
        record().optional_count(ctx).await
    }
}

struct Subscription;

#[async_graphql::Subscription]
impl Subscription {
    async fn record(&self) -> impl Stream<Item = DispatchRecord> {
        graphql_orm::futures::stream::once(async { record() })
    }
}

type Call = (&'static str, Option<&'static str>);

struct Policy(Arc<Mutex<Vec<Call>>>);

impl FieldPolicy for Policy {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a Context<'_>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        key: Option<&'static str>,
        record: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            assert_eq!(entity, "DispatchRecord");
            assert_eq!(
                record.unwrap().downcast_ref::<DispatchRecord>().unwrap().id,
                "a"
            );
            self.0.lock().unwrap().push((field, key));
            match ctx.data_opt::<Refusal>() {
                Some(Refusal::Deny) => Ok(false),
                Some(Refusal::Error) => Err(async_graphql::Error::new("Policy refused")
                    .extend_with(|_, extensions| {
                        extensions.set("code", "CUSTOM_REFUSAL");
                        extensions.set("reason", "explicit");
                    })),
                None => Ok(true),
            }
        })
    }

    fn can_write_field<'a>(
        &'a self,
        _: &'a Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

enum Refusal {
    Deny,
    Error,
}

fn assert_error(response: Response, message: &str, path: Vec<PathSegment>) {
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(response.errors[0].message, message);
    assert_eq!(response.errors[0].path, path);
}

#[tokio::test]
async fn ordinary_scalar_dispatch_preserves_metadata_values_and_policy_errors() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    db.set_field_policy(Policy(Arc::clone(&calls)));
    let schema = Schema::build(Query, EmptyMutation, Subscription)
        .data(db)
        .finish();

    let sdl = schema.sdl();
    assert!(sdl.contains("Dispatch record."), "{sdl}");
    assert!(sdl.contains("Payload."), "{sdl}");
    assert!(sdl.contains("payload: ScalarPayload!"), "{sdl}");
    assert!(sdl.contains("OptionalCount: Int"), "{sdl}");
    assert!(!sdl.contains("Omitted"), "{sdl}");

    let query = "{ list: records { output: payload nullable: OptionalCount } directGetter }";
    let response = schema.execute(query).await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"list": [{"output": {"count": 3}, "nullable": null}], "directGetter": null}),
    );
    let mut actual = std::mem::take(&mut *calls.lock().unwrap());
    actual.sort_unstable();
    assert_eq!(
        actual,
        [
            ("OptionalCount", Some("dispatch.optional")),
            ("OptionalCount", Some("dispatch.optional")),
            ("payload", None),
        ]
    );

    let document = "{\n  list: records { renamed: OptionalCount }\n}";
    for (refusal, code) in [
        (Refusal::Deny, "FORBIDDEN"),
        (Refusal::Error, "CUSTOM_REFUSAL"),
    ] {
        let response = schema
            .execute(async_graphql::Request::new(document).data(refusal))
            .await;
        assert_eq!(response.errors.len(), 1, "{response:?}");
        let error = &response.errors[0];
        assert_eq!(
            error.path,
            [
                PathSegment::Field("list".into()),
                PathSegment::Index(0),
                PathSegment::Field("renamed".into()),
            ]
        );
        assert_eq!(
            error.locations,
            [async_graphql::Pos {
                line: 2,
                column: 19
            }]
        );
        let extensions = error.extensions.as_ref().unwrap();
        assert_eq!(
            extensions.get("code"),
            Some(&async_graphql::Value::from(code))
        );
        if code == "CUSTOM_REFUSAL" {
            assert_eq!(error.message, "Policy refused");
            assert_eq!(
                extensions.get("reason"),
                Some(&async_graphql::Value::from("explicit"))
            );
        }
    }
    calls.lock().unwrap().clear();

    let request = "subscription { event: record { renamed: QueryOnly } }";
    let response = schema.execute_stream(request).next().await.unwrap();
    assert_error(
        response,
        "Field DispatchRecord.QueryOnly is not available in subscriptions",
        vec![
            PathSegment::Field("event".into()),
            PathSegment::Field("renamed".into()),
        ],
    );
    assert!(calls.lock().unwrap().is_empty());

    let response = schema
        .execute_stream("subscription { record { Id } }")
        .next()
        .await
        .unwrap();
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(*calls.lock().unwrap(), [("Id", None)]);
}

#[tokio::test]
async fn missing_database_precedes_scalar_subscription_check() {
    let schema = Schema::build(Query, EmptyMutation, Subscription).finish();
    let response = schema
        .execute_stream("subscription { record { alias: QueryOnly } }")
        .next()
        .await
        .unwrap();
    assert_error(
        response,
        super::generated_mutation_missing_database::DATABASE_ERROR,
        vec![
            PathSegment::Field("record".into()),
            PathSegment::Field("alias".into()),
        ],
    );
}
