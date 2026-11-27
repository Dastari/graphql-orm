// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! A generated Keyset root serves GraphQL requests only with `Complete` or
//! `Unrestricted` read visibility. Registration refuses it when the row policy
//! explicitly declares residual-only visibility, keeps it when the policy
//! leaves the capability unknown, and reports unknown roots as diagnostics.

use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::orm::{
    GeneratedGraphqlQueryShape, KEYSET_READ_VISIBILITY_REFUSAL_MESSAGE,
    KEYSET_READ_VISIBILITY_REFUSAL_REASON, KeysetReadPolicyReason, ReadPredicate, ReadVisibility,
    ReadVisibilityCapability,
};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "residual_items",
    plural = "ResidualItems",
    read_policy = "item.read",
    keyset = "id asc"
)]
struct ResidualItem {
    #[primary_key]
    #[sortable]
    #[filterable(type = "string")]
    #[graphql_orm(auto_generated = false)]
    id: String,
    label: String,
}

schema_roots! { query_custom_ops: [], entities: [ResidualItem], }

#[derive(Clone, Copy, Debug)]
enum Visibility {
    Callback,
    Prefilter,
    Complete,
}

fn visibility(mode: Visibility) -> async_graphql::Result<ReadVisibility> {
    let predicate = || {
        ReadPredicate::from_filter::<SqliteBackend, _>(&ResidualItemWhereInput {
            id: Some(StringFilter {
                eq: Some("one".into()),
                ..Default::default()
            }),
            ..Default::default()
        })
    };
    Ok(match mode {
        Visibility::Callback => ReadVisibility::CallbackOnly,
        Visibility::Prefilter => ReadVisibility::Prefilter(predicate()?),
        Visibility::Complete => ReadVisibility::Complete(predicate()?),
    })
}

struct AllowEntities;
impl EntityPolicy for AllowEntities {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

macro_rules! allow_rows {
    () => {
        fn can_read_row<'a>(
            &'a self,
            _: Option<&'a async_graphql::Context<'_>>,
            _: &'a Database,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessSurface,
            _: &'a (dyn std::any::Any + Send + Sync),
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
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
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
    };
}

/// Returns `returned` and leaves the capability at the trait default.
struct UndeclaredPolicy(Visibility);
impl RowPolicy for UndeclaredPolicy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        let mode = self.0;
        Box::pin(async move { visibility(mode) })
    }
    allow_rows!();
}

/// Declares `declared` and returns `returned` at runtime.
struct DeclaredPolicy {
    declared: ReadVisibilityCapability,
    returned: Visibility,
}
impl RowPolicy for DeclaredPolicy {
    fn read_visibility_capability(
        &self,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
    ) -> ReadVisibilityCapability {
        assert_eq!(
            (entity_name, policy_key),
            ("ResidualItem", Some("item.read"))
        );
        self.declared
    }
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        let mode = self.returned;
        Box::pin(async move { visibility(mode) })
    }
    allow_rows!();
}

fn declared(declared: ReadVisibilityCapability, returned: Visibility) -> DeclaredPolicy {
    DeclaredPolicy { declared, returned }
}

async fn database() -> Database {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE residual_items (id TEXT PRIMARY KEY, label TEXT NOT NULL)")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO residual_items VALUES ('one', 'visible'), ('hidden', 'hidden')")
        .execute(db.pool())
        .await
        .unwrap();
    db.set_entity_policy(AllowEntities);
    db
}

async fn database_with<P: RowPolicy + 'static>(policy: P) -> Database {
    let mut db = database().await;
    db.set_row_policy(policy);
    db
}

fn diagnostics(db: &Database) -> Vec<graphql_orm::graphql::orm::KeysetReadPolicyError> {
    db.keyset_read_policy_diagnostics(
        graphql_orm_operation_catalog(),
        &graphql_orm_entity_metadata(),
    )
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_default()
}

const KEYSET_QUERY: &str = "{ residualItemsKeyset(page: { limit: 2 }) { edges { node { id } } } }";

async fn keyset_ids(db: Database) -> Vec<String> {
    let schema = try_schema_builder(db)
        .expect("Keyset root registers")
        .data(graphql_orm::graphql::auth::AuthSubject::new("test-user"))
        .finish();
    assert!(schema.sdl().contains("residualItemsKeyset("));
    let response = schema.execute(KEYSET_QUERY).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    data["residualItemsKeyset"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"]["id"].as_str().unwrap().to_owned())
        .collect()
}

async fn assert_runtime_keyset_refusal(db: Database) {
    let schema = schema_builder(db)
        .data(graphql_orm::graphql::auth::AuthSubject::new("test-user"))
        .finish();
    let response = schema.execute(KEYSET_QUERY).await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    let error = &response.errors[0];
    assert_eq!(error.message, KEYSET_READ_VISIBILITY_REFUSAL_MESSAGE);
    let extensions = error.extensions.as_ref().unwrap();
    assert_eq!(
        extensions.get("code"),
        Some(&async_graphql::Value::from("AUTHORIZATION_MISCONFIGURED"))
    );
    assert_eq!(
        extensions.get("reason"),
        Some(&async_graphql::Value::from(
            KEYSET_READ_VISIBILITY_REFUSAL_REASON
        ))
    );
    // The public refusal says nothing about the policy itself.
    for private in ["item.read", "ResidualItem", "CallbackOnly", "Prefilter"] {
        assert!(!error.message.contains(private), "{}", error.message);
    }
    assert_eq!(response.data, async_graphql::Value::Null);
}

#[test]
fn the_catalog_marks_keyset_and_scan_roots_structurally() {
    let shape = |field| {
        graphql_orm_operation_catalog()
            .exposed_operations()
            .find(|operation| operation.field_name() == field)
            .and_then(|operation| operation.query_shape())
    };
    assert_eq!(
        shape("residualItemsKeyset"),
        Some(GeneratedGraphqlQueryShape::Keyset)
    );
    assert_eq!(
        shape("residualItemsScan"),
        Some(GeneratedGraphqlQueryShape::Scan)
    );
    assert_eq!(
        shape("residualItems"),
        Some(GeneratedGraphqlQueryShape::List)
    );
    assert_eq!(
        shape("residualItem"),
        Some(GeneratedGraphqlQueryShape::Single)
    );
}

#[test]
fn the_default_capability_is_unknown() {
    assert_eq!(
        UndeclaredPolicy(Visibility::Callback)
            .read_visibility_capability("ResidualItem", Some("item.read")),
        ReadVisibilityCapability::Unknown
    );
    assert_eq!(
        ReadVisibilityCapability::default(),
        ReadVisibilityCapability::Unknown
    );
}

/// Regression: a policy returning `Complete` with the default capability
/// keeps a working Keyset root, as on earlier releases.
#[tokio::test]
async fn complete_visibility_with_the_default_capability_builds_and_serves_keyset() {
    let db = database_with(UndeclaredPolicy(Visibility::Complete)).await;
    assert_eq!(keyset_ids(db).await, ["one"]);
}

#[tokio::test]
async fn explicit_residual_only_declaration_refuses_registration() {
    for returned in [Visibility::Callback, Visibility::Prefilter] {
        let db = database_with(declared(ReadVisibilityCapability::ResidualOnly, returned)).await;

        let error = db
            .validate_keyset_read_policies(
                graphql_orm_operation_catalog(),
                &graphql_orm_entity_metadata(),
            )
            .unwrap_err();
        assert_eq!(error.reason(), KeysetReadPolicyReason::DeclaredResidualOnly);
        assert!(error.refuses_registration());
        assert_eq!(error.field_name(), "residualItemsKeyset");
        assert_eq!(error.entity_name(), "ResidualItem");
        assert_eq!(error.policy_key(), Some("item.read"));
        let message = error.to_string();
        for expected in [
            "residualItemsKeyset",
            "ResidualItem",
            "item.read",
            "ResidualOnly",
            "query_shapes",
            "read_visibility_capability",
        ] {
            assert!(message.contains(expected), "{returned:?}: {message}");
        }
        assert_eq!(diagnostics(&db), std::slice::from_ref(&error));

        assert_eq!(try_schema_builder(db.clone()).err(), Some(error.clone()));
        assert_eq!(
            try_schema_builder_with_limits(db.clone(), SchemaLimits::default()).err(),
            Some(error.clone())
        );
        let panic =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| schema_builder(db.clone())))
                .err()
                .map(panic_message)
                .unwrap_or_else(|| panic!("{returned:?}: schema_builder registered the root"));
        assert_eq!(panic, message);
    }
}

#[tokio::test]
async fn unknown_capability_keeps_the_root_and_is_reported() {
    let db = database_with(UndeclaredPolicy(Visibility::Callback)).await;
    db.validate_keyset_read_policies(
        graphql_orm_operation_catalog(),
        &graphql_orm_entity_metadata(),
    )
    .unwrap();

    let found = diagnostics(&db);
    assert_eq!(found.len(), 1, "{found:?}");
    let finding = &found[0];
    assert_eq!(
        finding.reason(),
        KeysetReadPolicyReason::UndeclaredCapability
    );
    assert!(!finding.refuses_registration());
    assert_eq!(finding.field_name(), "residualItemsKeyset");
    assert_eq!(finding.entity_name(), "ResidualItem");
    assert_eq!(finding.policy_key(), Some("item.read"));
    let message = finding.to_string();
    assert!(message.contains("may be unusable"), "{message}");
    assert!(message.contains("read_visibility_capability"), "{message}");

    // The root stays registered; a residual request gets the documented refusal.
    let schema = try_schema_builder(db.clone()).unwrap().finish();
    assert!(schema.sdl().contains("residualItemsKeyset("));
    assert_runtime_keyset_refusal(db).await;
}

#[tokio::test]
async fn declared_complete_capability_is_not_reported_and_serves_keyset() {
    let db = database_with(declared(
        ReadVisibilityCapability::Complete,
        Visibility::Complete,
    ))
    .await;
    assert!(diagnostics(&db).is_empty());
    assert_eq!(keyset_ids(db).await, ["one"]);
}

#[tokio::test]
async fn declaration_never_bypasses_the_runtime_refusal() {
    for returned in [Visibility::Callback, Visibility::Prefilter] {
        let db = database_with(declared(ReadVisibilityCapability::Complete, returned)).await;
        assert!(diagnostics(&db).is_empty());
        assert_runtime_keyset_refusal(db).await;
    }
}

#[tokio::test]
async fn no_row_policy_refuses_a_keyed_keyset_root_outside_legacy_mode() {
    // A declared read key defers every row to `can_read_row`, which needs a
    // row provider outside `LegacyPermissive`.
    let db = database().await;
    let findings = diagnostics(&db);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(
        findings[0].reason(),
        KeysetReadPolicyReason::MissingRowPolicy
    );
    assert_eq!(findings[0].policy_key(), Some("item.read"));
    assert!(findings[0].refuses_registration());
    assert!(
        db.validate_keyset_read_policies(
            graphql_orm_operation_catalog(),
            &graphql_orm_entity_metadata()
        )
        .is_err()
    );
    assert!(try_schema_builder(db).is_err());
}

#[tokio::test]
async fn no_row_policy_in_legacy_mode_reports_nothing_and_serves_keyset() {
    let mut db = database().await;
    db.set_authorization_mode(AuthorizationMode::LegacyPermissive);
    assert!(diagnostics(&db).is_empty());
    assert_eq!(keyset_ids(db).await, ["hidden", "one"]);
}

#[tokio::test]
async fn missing_entity_metadata_is_an_error() {
    let without_policy = database().await;
    let with_policy = database_with(declared(
        ReadVisibilityCapability::Complete,
        Visibility::Complete,
    ))
    .await;
    for db in [without_policy, with_policy] {
        let error = db
            .validate_keyset_read_policies(graphql_orm_operation_catalog(), &[])
            .unwrap_err();
        assert_eq!(
            error.reason(),
            KeysetReadPolicyReason::MissingEntityMetadata
        );
        assert!(error.refuses_registration());
        assert_eq!(error.field_name(), "residualItemsKeyset");
        assert_eq!(error.entity_name(), "ResidualItem");
        assert_eq!(error.policy_key(), None);
        assert!(error.to_string().contains("missing"), "{error}");
    }
}

mod without_keyset_shape {
    use super::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql_entity(
        table = "residual_items",
        plural = "ResidualItems",
        read_policy = "item.read",
        keyset = "id asc",
        query_shapes = "single,list,scan"
    )]
    struct ResidualItem {
        #[primary_key]
        #[sortable]
        #[graphql_orm(auto_generated = false)]
        id: String,
        label: String,
    }

    schema_roots! { query_custom_ops: [], entities: [ResidualItem], }

    #[tokio::test]
    async fn residual_only_policy_builds_schema_without_keyset_shape() {
        let db = database_with(declared(
            ReadVisibilityCapability::ResidualOnly,
            Visibility::Callback,
        ))
        .await;
        assert!(
            db.keyset_read_policy_diagnostics(
                graphql_orm_operation_catalog(),
                &graphql_orm_entity_metadata(),
            )
            .is_empty()
        );
        let schema = try_schema_builder(db).unwrap().finish();
        let sdl = schema.sdl();
        assert!(!sdl.contains("residualItemsKeyset("), "{sdl}");
        assert!(sdl.contains("residualItemsScan("), "{sdl}");
        assert!(sdl.contains("residualItems("), "{sdl}");
    }
}
