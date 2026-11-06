---
title: graphql-orm usage examples
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-01
review_by: 2027-02-01
supersedes: []
---

# graphql-orm usage examples

Complete programs for the [`graphql-orm-usage` skill](SKILL.md). Each one runs in this repository's
tests.

## A write input transform

A transform shapes and validates input on every generated create, update and upsert path, so a client
can't skip it by choosing a different write. It runs once per input row. Bulk updates pass each
existing row to `before_update` with an independent copy of the original input; a refusal rolls back
the entire write. Implement `before_update` and `before_upsert` as needed. Field validators such as `#[graphql_orm(min_length = 2)]` are simpler
when they're enough.

```rust
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use graphql_orm::uuid::Uuid;
use std::any::Any;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "orders", plural = "Orders", default_sort = "reference ASC")]
struct Order {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "string")]
    #[sortable]
    reference: String,
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    query_custom_ops: [],
    entities: [Order],
}

/// Shapes and validates input on every generated create path. Keep it idempotent.
struct NormaliseOrders;

impl WriteInputTransform for NormaliseOrders {
    fn before_create<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        input: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if let Some(input) = input.downcast_mut::<CreateOrderInput>() {
                input.reference = input.reference.trim().to_uppercase();
                if input.reference.is_empty() {
                    return Err(async_graphql::Error::new("a reference is required"));
                }
            }
            Ok(())
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await?
        .with_schema_policy(SchemaPolicy::Managed);
    database.set_write_input_transform(NormaliseOrders);
    let plan = database
        .schema()
        .plan_migration_to_entities("001", "orders", &[Order::metadata()])
        .await?;
    database.schema().apply_migration(&plan, ApplyOptions::default()).await?;

    let order = Order::insert(&database, CreateOrderInput { reference: " a-1 ".into() }).await?;
    assert_eq!(order.reference, "A-1");
    assert!(Order::insert(&database, CreateOrderInput { reference: "  ".into() }).await.is_err());
    Ok(())
}
```

## A custom action with row policies

Sending an invoice again writes no entity, so it's a genuine action. As a custom resolver it inherits
no authorization, so it applies each check itself. The entity declares both policy keys, which needs an
entity provider and a row provider. Each provider sees every entity and every surface, so both state
what they do for in-process calls ([#291](https://github.com/Dastari/graphql-orm/issues/291)). A second
tenant is refused at the row check.

```rust
use async_graphql::{Context, Object, Request};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use graphql_orm::uuid::Uuid;
use std::any::Any;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "orders",
    plural = "Orders",
    default_sort = "reference ASC",
    read_policy = "order.read",
    write_policy = "order.write"
)]
struct Order {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "string")]
    #[sortable]
    reference: String,
    tenant_id: String,
}

/// Scope checks for requests. `Repository` is in-process code: hooks, workers and
/// the repository calls below. Allowing it trusts all of them, so each custom
/// resolver has to authorize before it gets that far.
struct Scopes;

impl EntityPolicy for Scopes {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let subject = ctx.and_then(|ctx| ctx.auth_subject_opt());
            Ok(match (surface, policy_key, subject) {
                (EntityAccessSurface::Repository, _, _) => true,
                (_, Some(key), Some(subject)) => subject.has_scope(key),
                _ => false,
            })
        })
    }
}

/// A row belongs to the caller's tenant.
struct TenantRows;

impl TenantRows {
    fn allows(
        ctx: Option<&Context<'_>>,
        surface: EntityAccessSurface,
        row: &(dyn Any + Send + Sync),
    ) -> bool {
        if surface == EntityAccessSurface::Repository {
            return true;
        }
        let tenant = ctx.and_then(|ctx| ctx.auth_subject_opt()).and_then(|subject| subject.tenant_id);
        let order = row.downcast_ref::<Order>();
        matches!((tenant, order), (Some(tenant), Some(order)) if tenant == order.tenant_id)
    }
}

impl RowPolicy for TenantRows {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(Self::allows(ctx, surface, row)) })
    }

    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(Self::allows(ctx, surface, row)) })
    }
}

#[derive(Default)]
struct OrderActions;

#[graphql_orm_custom_operations(kind = "mutation", authorization = true)]
#[Object]
impl OrderActions {
    /// Sends the invoice again. A genuine action: it writes no entity.
    async fn resend_invoice(&self, ctx: &Context<'_>, id: Uuid) -> async_graphql::Result<bool> {
        let db = ctx.data::<Database<SqliteBackend>>()?;
        let meta = Order::metadata();
        enforce_resolver_auth(ctx, None)?;
        db.ensure_entity_access(
            Some(ctx),
            meta.entity_name,
            meta.write_policy,
            EntityAccessKind::Write,
            EntityAccessSurface::GraphqlMutation,
        )
        .await?;
        // An in-process read: the row check below is what authorizes it.
        let order = Order::find_by_id(db, &id)
            .await
            .map_err(|error| async_graphql::Error::new(error.to_string()))?
            .ok_or_else(|| OrmPublicError::forbidden().into_graphql_error())?;
        db.ensure_writable_row(
            Some(ctx),
            meta.entity_name,
            meta.write_policy,
            EntityAccessSurface::GraphqlMutation,
            &order,
        )
        .await?;
        // Call the provider here.
        Ok(true)
    }
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    query_custom_ops: [],
    described_mutation_types: [OrderActions],
    entities: [Order],
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await?
        .with_schema_policy(SchemaPolicy::Managed);
    database.set_entity_policy(Scopes);
    database.set_row_policy(TenantRows);
    // Row callbacks need a scan budget for paged reads. Use a secret random key.
    let database = database.with_authorized_scan_config(AuthorizedScanConfig::new([7; 32], 256, 4096));
    let plan = database
        .schema()
        .plan_migration_to_entities("001", "orders", &[Order::metadata()])
        .await?;
    database.schema().apply_migration(&plan, ApplyOptions::default()).await?;
    let schema = schema_builder(database).finish();

    let subject = |tenant: &str| {
        AuthSubject::builder("user-1")
            .tenant_id(tenant)
            .scopes(vec!["order.read".into(), "order.write".into()])
            .build()
    };
    let create = r#"mutation { createOrder(input: { reference: "A-1", tenantId: "t1" }) { order { id } } }"#;
    let created = schema.execute(Request::new(create).data(subject("t1"))).await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let id = created.data.into_json()?["createOrder"]["order"]["id"].clone();

    let as_tenant = |tenant: &str| {
        Request::new(format!("mutation {{ resendInvoice(id: {id}) }}")).data(subject(tenant))
    };
    assert!(schema.execute(as_tenant("t1")).await.errors.is_empty());
    assert!(!schema.execute(as_tenant("t2")).await.errors.is_empty());

    // Paged reads return only the caller's rows.
    let list = "query { orders { edges { node { reference } } } }";
    let theirs = schema.execute(Request::new(list).data(subject("t2"))).await;
    assert!(theirs.errors.is_empty(), "{:?}", theirs.errors);
    assert_eq!(theirs.data.into_json()?["orders"]["edges"], serde_json::json!([]));
    Ok(())
}
```
