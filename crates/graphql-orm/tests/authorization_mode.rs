// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

use graphql_orm::graphql::orm::{EntityAccessKind, EntityAccessSurface};
use graphql_orm::prelude::*;
use std::sync::Arc;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "strict_docs",
    plural = "StrictDocs",
    default_sort = "name ASC",
    read_policy = "docs.read",
    write_policy = "docs.write",
    auth = "optional"
)]
struct StrictDoc {
    #[primary_key]
    pub id: String,
    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

schema_roots! {
    auth: "optional",
    query_custom_ops: [],
    entities: [StrictDoc],
}

#[derive(Clone, Default)]
struct AllowAllEntityPolicy;

impl EntityPolicy for AllowAllEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(true) })
    }
}

#[tokio::test]
async fn declared_policy_without_provider_fails_in_strict_mode()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    sqlx::query("CREATE TABLE strict_docs (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO strict_docs (id, name) VALUES ('1', 'alpha')")
        .execute(&pool)
        .await?;

    let database = Database::new(pool);
    assert_eq!(
        database.authorization_mode(),
        AuthorizationMode::DeclaredPoliciesRequired
    );
    let schema = schema_builder(database)
        .data(AuthSubject::new("user-1"))
        .finish();

    let response = schema
        .execute("{ strictDocs { edges { node { id name } } } }")
        .await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    assert_eq!(response.errors[0].message, "authorization is misconfigured");
    let extensions = response.errors[0].extensions.as_ref().expect("extensions");
    assert_eq!(
        extensions.get("code").map(|value| value.to_string()),
        Some("\"AUTHORIZATION_MISCONFIGURED\"".to_string())
    );
    Ok(())
}

#[tokio::test]
async fn legacy_mode_allows_declared_policy_without_provider()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    sqlx::query("CREATE TABLE strict_docs (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO strict_docs (id, name) VALUES ('1', 'alpha')")
        .execute(&pool)
        .await?;

    let database = Database::new(pool).with_authorization_mode(AuthorizationMode::LegacyPermissive);
    let schema = schema_builder(database)
        .data(AuthSubject::new("user-1"))
        .finish();

    let response = schema
        .execute("{ strictDocs { edges { node { id name } } } }")
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    Ok(())
}

#[tokio::test]
async fn strict_mode_succeeds_when_provider_registered() -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    sqlx::query("CREATE TABLE strict_docs (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO strict_docs (id, name) VALUES ('1', 'alpha')")
        .execute(&pool)
        .await?;

    let mut database =
        Database::new(pool).with_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
    database.set_entity_policy(AllowAllEntityPolicy);
    let query = "{ strictDocs { edges { node { id name } } } strictDoc(id: \"1\") { name } }";

    // The entity read key is also its row key: lists and point reads both
    // need a row provider.
    let response = schema_builder(database.clone())
        .data(AuthSubject::new("user-1"))
        .finish()
        .execute(query)
        .await;
    assert!(
        response
            .errors
            .iter()
            .any(|error| error.message == "authorization is misconfigured"),
        "{:?}",
        response.errors
    );

    database.set_row_policy(AllowAllRowPolicy);
    let response = schema_builder(database)
        .data(AuthSubject::new("user-1"))
        .finish()
        .execute(query)
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json()?;
    assert_eq!(json["strictDocs"]["edges"][0]["node"]["name"], "alpha");
    assert_eq!(json["strictDoc"]["name"], "alpha");
    Ok(())
}

struct AllowAllRowPolicy;

impl RowPolicy for AllowAllRowPolicy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async { Ok(ReadVisibility::Unrestricted) })
    }

    fn can_read_row<'a>(
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

#[tokio::test]
async fn explicit_mode_denies_without_entity_policy() -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let database = Arc::new(
        Database::new(pool)
            .with_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations),
    );
    let allowed = database
        .can_access_entity(
            None,
            "StrictDoc",
            None,
            EntityAccessKind::Read,
            EntityAccessSurface::GraphqlQuery,
        )
        .await
        .expect("policy evaluation should not error");
    assert!(!allowed);
    Ok(())
}

#[test]
fn pagination_defaults_are_secure() {
    assert_eq!(PaginationConfig::DEFAULT_LIMIT, 50);
    assert_eq!(PaginationConfig::DEFAULT_MAX_LIMIT, 100);
    let legacy = PaginationConfig::legacy();
    assert_eq!(legacy.default_limit, Some(1000));
    assert_eq!(legacy.max_limit, Some(1000));
}

#[test]
fn structural_auth_required_tenant() {
    let metadata =
        StructuralAuthMetadata::new(Some("tenant_id"), None, StructuralAuthorization::Required);
    let denied = resolve_structural_auth(metadata, &StructuralAuthValues::default());
    assert!(matches!(
        denied,
        StructuralAuthResolution::DeniedMissingContext
    ));

    let allowed = resolve_structural_auth(
        metadata,
        &StructuralAuthValues::new(Some("tenant-a".into()), None),
    );
    assert!(matches!(allowed, StructuralAuthResolution::Filter(_)));
}

#[test]
fn public_errors_hide_sql() {
    let error = OrmPublicError::internal("relation \"users\" does not exist").into_graphql_error();
    assert_eq!(error.message, "internal error");
    assert!(!format!("{error:?}").contains("users"));
}
