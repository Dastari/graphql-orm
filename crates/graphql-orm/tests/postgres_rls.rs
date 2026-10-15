#![cfg(feature = "postgres")]

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use async_graphql::{Request, Schema, SimpleObject};
use graphql_orm::graphql::orm::{
    DatabaseBackend, DbAuthContext, PostgresBackend, SchemaPolicy,
    apply_db_auth_context_to_transaction, build_rls_policy_plan, fetch_rows_with_auth,
    postgres_rls_helper_sql,
};
use graphql_orm::prelude::*;
use sqlx::{Acquire, Row};

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_accounts",
    plural = "RlsAccounts",
    default_sort = "id ASC"
)]
#[graphql(complex)]
struct RlsAccount {
    #[primary_key]
    pub id: String,

    #[sortable]
    pub name: String,

    #[graphql(skip)]
    #[relation(target = "RlsNote", from = "id", to = "account_id", multiple)]
    pub notes: Vec<RlsNote>,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "rls_notes",
    plural = "RlsNotes",
    default_sort = "id ASC"
)]
#[graphql_rls(select(scope = "notes.read", tenant = "tenant_id"))]
struct RlsNote {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    pub account_id: String,

    #[filterable(type = "string")]
    pub tenant_id: String,

    #[sortable]
    pub title: String,
}

impl BatchLoadEntity for RlsNote {
    fn batch_column() -> &'static str {
        "account_id"
    }

    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("account_id")
    }
}

schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [RlsAccount, RlsNote],
}

type RlsSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

fn auth_context(tenant_id: &str, scopes: &[&str]) -> DbAuthContext {
    DbAuthContext {
        user_id: Some("user-1".to_string()),
        subject: Some("user-1".to_string()),
        tenant_id: Some(tenant_id.to_string()),
        roles: vec!["member".to_string()],
        scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
        claims_json: Some(serde_json::json!({"tenant": tenant_id})),
        ..Default::default()
    }
}

async fn test_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
    owned_postgres::OwnedPostgres::pool("postgres-rls", 5).await
}

async fn install_rls_helpers(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    for statement in postgres_rls_helper_sql() {
        sqlx::query(&statement).execute(pool).await?;
    }
    Ok(())
}

#[test]
fn db_auth_context_serializes_deterministically() {
    let left = DbAuthContext {
        user_id: Some("u1".to_string()),
        subject: Some("u1".to_string()),
        tenant_id: Some("t1".to_string()),
        roles: vec!["writer".to_string(), "reader".to_string()],
        scopes: vec!["notes.write".to_string(), "notes.read".to_string()],
        claims_json: Some(serde_json::json!({"b": 2, "a": 1})),
        ..Default::default()
    };
    let right = DbAuthContext {
        roles: vec!["reader".to_string(), "writer".to_string()],
        scopes: vec!["notes.read".to_string(), "notes.write".to_string()],
        ..left.clone()
    };

    assert_eq!(left.canonical_key(), right.canonical_key());

    let settings = left.postgres_settings().expect("settings");
    assert_eq!(settings[0], ("app.user_id", "u1".to_string()));
    assert_eq!(
        settings[3],
        ("app.roles", r#"["writer","reader"]"#.to_string())
    );
    assert_eq!(
        settings[4],
        ("app.scopes", r#"["notes.write","notes.read"]"#.to_string())
    );
    assert_eq!(settings[5].0, "app.claims");
    assert!(settings[5].1.contains("\"a\":1"));
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn missing_auth_context_preserves_plain_postgres_fetch()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = test_pool().await?;

    let rows =
        fetch_rows_with_auth::<PostgresBackend>(&pool, "SELECT 42::BIGINT AS value", &[], None)
            .await?;
    let value: i64 = rows[0].try_get("value")?;
    assert_eq!(value, 42);
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn transaction_local_settings_are_visible_and_do_not_leak()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = test_pool().await?;
    install_rls_helpers(&pool).await?;

    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    let auth = auth_context("tenant-a", &["notes.read"]);
    apply_db_auth_context_to_transaction::<PostgresBackend>(&mut tx, Some(&auth)).await?;

    let inside = sqlx::query(
        "SELECT graphql_orm.current_user_id() AS user_id,
                graphql_orm.current_tenant_id() AS tenant_id,
                graphql_orm.current_scopes() AS scopes",
    )
    .fetch_one(&mut *tx)
    .await?;
    let user_id: Option<String> = inside.try_get("user_id")?;
    let tenant_id: Option<String> = inside.try_get("tenant_id")?;
    let scopes: Vec<String> = inside.try_get("scopes")?;
    assert_eq!(user_id.as_deref(), Some("user-1"));
    assert_eq!(tenant_id.as_deref(), Some("tenant-a"));
    assert_eq!(scopes, vec!["notes.read".to_string()]);
    tx.commit().await?;

    let outside = sqlx::query("SELECT current_setting('app.user_id', true) AS user_id")
        .fetch_one(&mut *conn)
        .await?;
    let leaked: Option<String> = outside.try_get("user_id")?;
    assert!(leaked.as_deref().unwrap_or_default().is_empty());
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn concurrent_auth_contexts_do_not_cross_contaminate()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = test_pool().await?;
    install_rls_helpers(&pool).await?;

    async fn read_tenant(
        pool: &sqlx::PgPool,
        tenant_id: &'static str,
    ) -> Result<String, sqlx::Error> {
        let auth = auth_context(tenant_id, &["notes.read"]);
        let mut tx = pool.begin().await?;
        apply_db_auth_context_to_transaction::<PostgresBackend>(&mut tx, Some(&auth)).await?;
        let row =
            sqlx::query("SELECT pg_sleep(0.05), graphql_orm.current_tenant_id() AS tenant_id")
                .fetch_one(&mut *tx)
                .await?;
        tx.commit().await?;
        row.try_get("tenant_id")
    }

    let (left, right) = tokio::join!(
        read_tenant(&pool, "tenant-a"),
        read_tenant(&pool, "tenant-b")
    );
    assert_eq!(left?, "tenant-a");
    assert_eq!(right?, "tenant-b");
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn generated_queries_and_relation_preload_obey_live_rls()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = test_pool().await?;

    sqlx::query("DROP TABLE IF EXISTS rls_notes")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS rls_accounts")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE TABLE rls_accounts (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE rls_notes (
            id TEXT PRIMARY KEY,
            account_id TEXT NOT NULL REFERENCES rls_accounts(id),
            tenant_id TEXT NOT NULL,
            title TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO rls_accounts (id, name) VALUES ('a1', 'Account')")
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO rls_notes (id, account_id, tenant_id, title)
         VALUES ('n1', 'a1', 'tenant-a', 'Tenant A')",
    )
    .execute(&pool)
    .await?;

    let target = graphql_orm_schema_target();
    let plan = build_rls_policy_plan(
        DatabaseBackend::Postgres,
        SchemaPolicy::Managed,
        &target.rls,
    );
    for statement in plan.statements {
        sqlx::query(&statement).execute(&pool).await?;
    }

    // The container owner provisions the fixture; actual reads must use a
    // different role that cannot bypass RLS or inherit table-owner privileges.
    let token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
    let role = format!("rls_reader_{token}");
    let password = format!("synthetic_{token}");
    sqlx::query(&format!(
        "CREATE ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '{password}'"
    ))
    .execute(&pool)
    .await?;
    sqlx::query(&format!("GRANT USAGE ON SCHEMA public TO {role}"))
        .execute(&pool)
        .await?;
    sqlx::query(&format!(
        "GRANT SELECT ON rls_accounts, rls_notes TO {role}"
    ))
    .execute(&pool)
    .await?;
    let options = pool
        .connect_options()
        .as_ref()
        .clone()
        .username(&role)
        .password(&password);
    let reader = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    let can_bypass: bool = sqlx::query_scalar(
        "SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&reader)
    .await?;
    assert!(
        !can_bypass,
        "RLS assertions must execute as a restricted reader"
    );

    let schema: RlsSchema = schema_builder(graphql_orm::db::Database::new(reader))
        .data("test-user".to_string())
        .finish();

    let visible = schema
        .execute(
            Request::new(
                "{ rlsNotes { edges { node { id title tenantId } } } rlsNote(id: \"n1\") { id title } }",
            )
            .data(auth_context("tenant-a", &["notes.read"])),
        )
        .await;
    assert!(visible.errors.is_empty(), "{:?}", visible.errors);
    let visible = visible.data.into_json()?;
    assert_eq!(visible["rlsNotes"]["edges"].as_array().unwrap().len(), 1);
    assert_eq!(
        visible["rlsNotes"]["edges"][0]["node"]["title"].as_str(),
        Some("Tenant A")
    );
    assert_eq!(visible["rlsNote"]["title"].as_str(), Some("Tenant A"));

    let wrong_tenant = schema
        .execute(
            Request::new("{ rlsNotes { edges { node { id } } } rlsNote(id: \"n1\") { id } }")
                .data(auth_context("tenant-b", &["notes.read"])),
        )
        .await;
    assert!(wrong_tenant.errors.is_empty(), "{:?}", wrong_tenant.errors);
    let wrong_tenant = wrong_tenant.data.into_json()?;
    assert_eq!(
        wrong_tenant["rlsNotes"]["edges"]
            .as_array()
            .expect("edges")
            .len(),
        0
    );
    assert!(wrong_tenant["rlsNote"].is_null());

    let missing_scope = schema
        .execute(
            Request::new("{ rlsNotes { edges { node { id } } } }")
                .data(auth_context("tenant-a", &[])),
        )
        .await;
    assert!(
        missing_scope.errors.is_empty(),
        "{:?}",
        missing_scope.errors
    );
    let missing_scope = missing_scope.data.into_json()?;
    assert_eq!(
        missing_scope["rlsNotes"]["edges"]
            .as_array()
            .expect("edges")
            .len(),
        0
    );

    let relation = schema
        .execute(
            Request::new(
                "{ rlsAccounts { edges { node { id notes { edges { node { id title tenantId } } } } } } }",
            )
            .data(auth_context("tenant-a", &["notes.read"])),
        )
        .await;
    assert!(relation.errors.is_empty(), "{:?}", relation.errors);
    let relation = relation.data.into_json()?;
    let notes = relation["rlsAccounts"]["edges"][0]["node"]["notes"]["edges"]
        .as_array()
        .expect("relation notes");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["node"]["title"].as_str(), Some("Tenant A"));
    Ok(())
}
