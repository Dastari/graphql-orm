#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use std::any::Any;

macro_rules! single_entity {
    ($name:ident, $table:literal, $append:literal) => {
        #[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
        #[graphql_entity(table = $table, append_only = $append, auth = "none", read_policy = "audit.read", notify_with = "notify")]
        struct $name {
            #[primary_key]
            #[graphql_orm(auto_generated = false)]
            #[sortable]
            id: String,
            #[graphql_orm(write_policy = "payload.write")]
            payload: String,
            #[graphql_orm(write_policy = "optional.write")]
            optional: Option<String>,
        }
    };
}
single_entity!(OrdinaryAudit, "ordinary_audit", false);
single_entity!(AppendAudit, "append_audit", true);

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "composite_audit",
    append_only = true,
    auth = "none",
    read_policy = "audit.read",
    notify_with = "notify"
)]
struct CompositeAudit {
    #[primary_key]
    #[sortable]
    tenant: String,
    #[primary_key]
    sequence: i32,
    #[graphql_orm(write_policy = "payload.write")]
    payload: String,
    #[graphql_orm(write_policy = "optional.write")]
    optional: Option<String>,
}

schema_roots! { query_custom_ops: [], entities: [OrdinaryAudit, AppendAudit, CompositeAudit] }

#[derive(Clone, Copy)]
struct Policy {
    field: bool,
    optional: bool,
    entity: bool,
    row: bool,
    fail_field: bool,
}
impl FieldPolicy for Policy {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        policy: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        value: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            assert!(value.is_some());
            if self.fail_field {
                return Err(std::io::Error::other("private provider details").into());
            }
            if field == "optional" {
                assert_eq!(policy, Some("optional.write"));
                record(ctx, "field:optional");
                Ok(self.optional)
            } else {
                assert_eq!(policy, Some("payload.write"));
                let payload = value
                    .and_then(|value| value.downcast_ref::<String>())
                    .unwrap();
                record(ctx, &format!("field:{field}:{payload}"));
                Ok(self.field && payload != "denied")
            }
        })
    }
}
impl EntityPolicy for Policy {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if kind == EntityAccessKind::Read {
                assert!(ctx.is_some());
                assert_eq!(surface, EntityAccessSurface::GraphqlMutation);
                assert_eq!(key, Some("audit.read"));
                record(ctx.unwrap(), "read:entity");
                if !self.entity {
                    ctx.unwrap().add_error(async_graphql::ServerError::new(
                        "private entity diagnostic",
                        None,
                    ));
                }
                Ok(self.entity)
            } else {
                Ok(true)
            }
        })
    }
}
impl RowPolicy for Policy {
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            assert!(ctx.is_some());
            assert_eq!(surface, EntityAccessSurface::GraphqlMutation);
            assert_eq!(key, Some("audit.read"));
            record(ctx.unwrap(), "read:row");
            assert!(
                row.is::<OrdinaryAudit>() || row.is::<AppendAudit>() || row.is::<CompositeAudit>()
            );
            if !self.row {
                ctx.unwrap().add_error(async_graphql::ServerError::new(
                    "private row diagnostic",
                    None,
                ));
            }
            Ok(self.row)
        })
    }
}

const ALLOW: Policy = Policy {
    field: true,
    optional: true,
    entity: true,
    row: true,
    fail_field: false,
};

async fn database() -> graphql_orm::Result<Database> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    for table in ["ordinary_audit", "append_audit"] {
        sqlx::query(&format!(
            "CREATE TABLE {table} (id TEXT PRIMARY KEY, payload TEXT NOT NULL, optional TEXT)"
        ))
        .execute(db.pool())
        .await?;
    }
    sqlx::query("CREATE TABLE composite_audit (tenant TEXT, sequence INTEGER, payload TEXT NOT NULL, optional TEXT, PRIMARY KEY (tenant, sequence))").execute(db.pool()).await?;
    Ok(db)
}

// Each scenario runs against ordinary, single-key append-only and composite-key
// append-only creates, comparing the public outcome and durable write count.
async fn check(
    policy: Policy,
    providers: [bool; 3],
    expected_code: Option<&str>,
    committed: bool,
    disclosed: bool,
) -> graphql_orm::Result<()> {
    let mut db = database().await?;
    if providers[0] {
        db.set_field_policy(policy);
    }
    if providers[1] {
        db.set_entity_policy(policy);
    }
    if providers[2] {
        db.set_row_policy(policy);
    }
    let schema = schema_builder(db.clone()).finish();
    for (name, table, keys) in [
        ("OrdinaryAudit", "ordinary_audit", r#"id: "one""#),
        ("AppendAudit", "append_audit", r#"id: "one""#),
        (
            "CompositeAudit",
            "composite_audit",
            r#"tenant: "one", sequence: 2"#,
        ),
    ] {
        let field = format!("{}{}", name[..1].to_lowercase(), &name[1..]);
        let response = schema.execute(format!(r#"mutation {{ create{name}(input: {{ {keys}, payload: "protected", optional: "optional" }}) {{ success {field} {{ payload optional }} }} }}"#)).await;
        if let Some(code) = expected_code {
            assert_eq!(response.errors.len(), 1, "{name}: {response:?}");
            let extensions = response.errors[0]
                .extensions
                .as_ref()
                .unwrap_or_else(|| panic!("{name}/{code}: {response:?}"));
            assert_eq!(
                extensions.get("code"),
                Some(&async_graphql::Value::from(code)),
                "{name}: {response:?}"
            );
            if committed {
                assert_eq!(
                    extensions.get("outcome"),
                    Some(&async_graphql::Value::from("committed"))
                );
                assert_eq!(
                    extensions.get("retryable"),
                    Some(&async_graphql::Value::from(false))
                );
            }
            assert!(!format!("{:?}", response.data).contains("protected"));
            assert!(
                !response.errors[0]
                    .message
                    .contains("private provider details")
            );
        } else {
            assert!(response.errors.is_empty(), "{name}: {response:?}");
            let data = response.data.into_json().unwrap();
            let result = &data[format!("create{name}")];
            assert_eq!(result["success"], true);
            assert_eq!(result[&field].is_null(), !disclosed, "{name}: {data}");
            if disclosed {
                assert_eq!(result[&field]["payload"], "protected");
            }
        }
        let count: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count.0, i64::from(committed), "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn create_field_write_authorization_matches_ordinary_creates() -> graphql_orm::Result<()> {
    check(
        ALLOW,
        [false, true, true],
        Some("AUTHORIZATION_MISCONFIGURED"),
        false,
        false,
    )
    .await?;
    check(
        Policy {
            field: false,
            ..ALLOW
        },
        [true; 3],
        Some("FORBIDDEN"),
        false,
        false,
    )
    .await?;
    check(
        Policy {
            optional: false,
            ..ALLOW
        },
        [true; 3],
        Some("FORBIDDEN"),
        false,
        false,
    )
    .await?;
    check(
        Policy {
            fail_field: true,
            ..ALLOW
        },
        [true; 3],
        Some("INTERNAL_ERROR"),
        false,
        false,
    )
    .await?;
    check(ALLOW, [true; 3], None, true, true).await
}

#[tokio::test]
async fn create_output_authorization_matches_ordinary_creates() -> graphql_orm::Result<()> {
    check(
        ALLOW,
        [true, false, true],
        Some("AUTHORIZATION_MISCONFIGURED"),
        true,
        false,
    )
    .await?;
    check(
        ALLOW,
        [true, true, false],
        Some("AUTHORIZATION_MISCONFIGURED"),
        true,
        false,
    )
    .await?;
    check(
        Policy {
            entity: false,
            ..ALLOW
        },
        [true; 3],
        Some("FORBIDDEN"),
        true,
        false,
    )
    .await?;
    check(
        Policy {
            row: false,
            ..ALLOW
        },
        [true; 3],
        None,
        true,
        false,
    )
    .await?;
    check(ALLOW, [true; 3], None, true, true).await
}

#[tokio::test]
async fn status_only_creates_do_not_require_output_read_providers() -> graphql_orm::Result<()> {
    let mut db = database().await?;
    db.set_field_policy(ALLOW);
    let schema = schema_builder(db.clone()).finish();
    for (name, table, keys) in [
        ("OrdinaryAudit", "ordinary_audit", r#"id: "one""#),
        ("AppendAudit", "append_audit", r#"id: "one""#),
        (
            "CompositeAudit",
            "composite_audit",
            r#"tenant: "one", sequence: 2"#,
        ),
    ] {
        let response = schema.execute(format!(r#"mutation {{ create{name}(input: {{ {keys}, payload: "protected" }}) {{ success error }} }}"#)).await;
        assert!(response.errors.is_empty(), "{name}: {response:?}");
        assert_eq!(
            response.data.into_json().unwrap()[format!("create{name}")]["success"],
            true
        );
        let count: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count.0, 1);
    }
    Ok(())
}

#[tokio::test]
async fn aliased_fragment_output_still_requires_read_authority() -> graphql_orm::Result<()> {
    let mut db = database().await?;
    db.set_field_policy(ALLOW);
    let schema = schema_builder(db).finish();
    for (name, field, keys) in [
        ("OrdinaryAudit", "ordinaryAudit", r#"id: "one""#),
        ("AppendAudit", "appendAudit", r#"id: "one""#),
        (
            "CompositeAudit",
            "compositeAudit",
            r#"tenant: "one", sequence: 2"#,
        ),
    ] {
        let response = schema.execute(format!(r#"mutation {{ create{name}(input: {{ {keys}, payload: "protected" }}) {{ ...Output }} }} fragment Output on {name}Result {{ renamed: {field} {{ payload }} }}"#)).await;
        assert_eq!(response.errors.len(), 1, "{name}: {response:?}");
        assert_eq!(
            response.errors[0].extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from("AUTHORIZATION_MISCONFIGURED"))
        );
        assert!(!format!("{:?}", response.data).contains("protected"));
    }
    Ok(())
}

#[derive(Default)]
struct Trace(std::sync::Mutex<Vec<String>>);

#[derive(Clone)]
struct RequestData {
    payload: &'static str,
    fail_after: bool,
}

fn record(ctx: &async_graphql::Context<'_>, event: &str) {
    if let Some(trace) = ctx.data_opt::<std::sync::Arc<Trace>>() {
        trace.0.lock().unwrap().push(event.to_owned());
    }
}

async fn notify<T: Entity + serde::Serialize>(
    ctx: &async_graphql::Context<'_>,
    pool: &sqlx::SqlitePool,
    _: &T,
    action: ChangeAction,
) -> async_graphql::Result<()> {
    if ctx.data_opt::<std::sync::Arc<Trace>>().is_some() {
        assert_eq!(action, ChangeAction::Created);
        let count: (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM {}",
            T::metadata().table_name
        ))
        .fetch_one(pool)
        .await?;
        assert_eq!(count.0, 1, "notification must observe committed storage");
        record(ctx, "notify");
    }
    Ok(())
}

struct Transform;
impl WriteInputTransform for Transform {
    fn before_create_with_context<'a>(
        &'a self,
        write: &'a mut WriteInputContext<'_, '_>,
        input: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            assert_eq!(write.origin(), WriteOrigin::GraphqlMutation);
            let ctx = write.graphql_ctx().expect("GraphQL before_create context");
            let request = ctx.data::<RequestData>()?;
            record(ctx, "transform");
            if let Some(input) = input.downcast_mut::<CreateOrdinaryAuditInput>() {
                input.payload = request.payload.into();
            } else if let Some(input) = input.downcast_mut::<CreateAppendAuditInput>() {
                input.payload = request.payload.into();
            } else {
                input
                    .downcast_mut::<CreateCompositeAuditInput>()
                    .unwrap()
                    .payload = request.payload.into();
            }
            Ok(())
        })
    }
}

struct Hook;
impl MutationHook for Hook {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            let ctx = ctx.expect("GraphQL mutation hook context");
            let request = ctx.data::<RequestData>()?;
            assert_eq!(event.action, ChangeAction::Created);
            record(ctx, &format!("hook:{:?}", event.phase));
            if event.phase == MutationPhase::After && request.fail_after {
                return Err(std::io::Error::other("private transaction hook failure").into());
            }
            Ok(())
        })
    }
}

const SHAPES: [(&str, &str, &str, &str); 3] = [
    (
        "OrdinaryAudit",
        "ordinary_audit",
        "ordinaryAudit",
        r#"id: "one""#,
    ),
    ("AppendAudit", "append_audit", "appendAudit", r#"id: "one""#),
    (
        "CompositeAudit",
        "composite_audit",
        "compositeAudit",
        r#"tenant: "one", sequence: 2"#,
    ),
];

async fn pipeline(request: RequestData, sql_failure: bool) -> graphql_orm::Result<()> {
    let mut db = database().await?;
    db.set_field_policy(ALLOW);
    db.set_entity_policy(ALLOW);
    db.set_row_policy(ALLOW);
    db.set_write_input_transform(Transform);
    db.set_mutation_hook(Hook);
    if sql_failure {
        for (_, table, _, _) in SHAPES {
            sqlx::query(&format!(
                "ALTER TABLE {table} RENAME COLUMN payload TO stored_payload"
            ))
            .execute(db.pool())
            .await?;
        }
    }
    let schema = schema_builder(db.clone()).finish();
    let mut ordinary_error = None;
    for (name, table, field, keys) in SHAPES {
        let trace = std::sync::Arc::new(Trace::default());
        let response = schema.execute(async_graphql::Request::new(format!(
            r#"mutation {{ create{name}(input: {{ {keys}, payload: "allowed" }}) {{ success error {field} {{ payload }} }} }}"#
        )).data(request.clone()).data(trace.clone())).await;
        let denied = request.payload == "denied";
        let failed = denied || request.fail_after || sql_failure;
        let count: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count.0, i64::from(!failed), "{name}: {response:?}");
        let observed = trace.0.lock().unwrap().clone();
        let mut expected = vec![
            "transform".to_owned(),
            format!("field:payload:{}", request.payload),
        ];
        if !denied {
            expected.extend(["field:optional", "hook:Before"].map(str::to_owned));
            if !sql_failure {
                expected.push("hook:After".into());
            }
            if !failed {
                expected.extend(["read:entity", "read:row", "notify"].map(str::to_owned));
            }
        }
        assert_eq!(observed, expected, "{name}");
        if failed {
            assert_eq!(response.errors.len(), 1, "{name}: {response:?}");
            let error = &response.errors[0];
            assert_eq!(
                error.extensions.as_ref().unwrap().get("code"),
                Some(&async_graphql::Value::from(if denied {
                    "FORBIDDEN"
                } else {
                    "INTERNAL_ERROR"
                })),
                "{name}: {response:?}"
            );
            assert!(!error.message.contains("private transaction"));
            assert!(!error.message.contains("stored_payload"));
            assert_eq!(response.data, async_graphql::Value::Null);
            let public_error = (error.message.clone(), error.extensions.clone());
            if let Some(expected) = &ordinary_error {
                assert_eq!(&public_error, expected, "{name}");
            } else {
                ordinary_error = Some(public_error);
            }
        } else {
            assert!(response.errors.is_empty(), "{name}: {response:?}");
            let data = response.data.into_json().unwrap();
            assert_eq!(data[format!("create{name}")]["success"], true);
            assert_eq!(
                data[format!("create{name}")][field]["payload"],
                request.payload
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn transformed_values_are_authorized_before_insert_in_every_create_pipeline()
-> graphql_orm::Result<()> {
    pipeline(
        RequestData {
            payload: "denied",
            fail_after: false,
        },
        false,
    )
    .await
}

#[tokio::test]
async fn request_context_hooks_commit_output_and_notifications_have_the_same_order()
-> graphql_orm::Result<()> {
    pipeline(
        RequestData {
            payload: "shaped",
            fail_after: false,
        },
        false,
    )
    .await
}

#[tokio::test]
async fn transaction_hook_errors_are_structured_and_roll_back_without_notifying()
-> graphql_orm::Result<()> {
    pipeline(
        RequestData {
            payload: "shaped",
            fail_after: true,
        },
        false,
    )
    .await
}

#[tokio::test]
async fn sql_transaction_errors_have_the_same_public_category_and_extensions()
-> graphql_orm::Result<()> {
    pipeline(
        RequestData {
            payload: "shaped",
            fail_after: false,
        },
        true,
    )
    .await
}
