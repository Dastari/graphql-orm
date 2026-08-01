#![cfg(feature = "sqlite")]

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use graphql_orm::async_graphql::ErrorExtensions;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "assurance_records",
    plural = "AssuranceRecords"
)]
struct AssuranceRecord {
    #[primary_key]
    #[filterable(type = "uuid")]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    value: String,
}

#[derive(Default)]
struct CustomAssuranceMutations;

#[graphql_orm::async_graphql::Object]
impl CustomAssuranceMutations {
    #[graphql(guard = "DeclaredAssuranceGuard::new(GraphqlOperationKind::Mutation)")]
    async fn service_heartbeat(&self) -> bool {
        true
    }
}

schema_roots! {
    backend: "sqlite",
    entities: [AssuranceRecord],
    extra_mutation_types: [CustomAssuranceMutations],
}

#[derive(Clone)]
struct DenyingEvaluator {
    calls: Arc<AtomicUsize>,
}

impl fmt::Debug for DenyingEvaluator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DenyingEvaluator").finish()
    }
}

impl AssuranceRequirementEvaluator for DenyingEvaluator {
    fn enforce(
        &self,
        _ctx: &graphql_orm::async_graphql::Context<'_>,
        actor_class: AssuranceActorClass,
        policy_id: &str,
    ) -> graphql_orm::async_graphql::Result<()> {
        assert_eq!(actor_class, AssuranceActorClass::Interactive);
        assert_eq!(policy_id, "interactive.recent-auth");
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(
            graphql_orm::async_graphql::Error::new("additional authentication is required")
                .extend_with(|_, extensions| {
                    extensions.set("code", "STEP_UP_REQUIRED");
                }),
        )
    }
}

fn registry() -> OperationAssuranceRegistry {
    let config = AssuranceSchemaConfig::legacy()
        .with_default_interactive_mutation_policy("interactive.recent-auth")
        .unwrap()
        .with_strict_mutation_classification(true);
    let mut builder = OperationAssuranceRegistry::builder(graphql_orm_operation_catalog(), config);
    builder
        .register_custom(
            "custom:service-heartbeat:v1",
            GraphqlOperationKind::Mutation,
            "serviceHeartbeat",
            AssuranceActorClass::Service,
        )
        .unwrap()
        .exempt(
            GraphqlOperationKind::Mutation,
            "serviceHeartbeat",
            "service operation uses a non-interactive principal",
        )
        .unwrap();
    builder.build().unwrap()
}

#[tokio::test]
async fn generated_mutation_is_guarded_and_explicit_service_exemption_executes()
-> Result<(), Box<dyn std::error::Error>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let enforcement = AssuranceEnforcement::new(
        Arc::new(registry()),
        Arc::new(DenyingEvaluator {
            calls: calls.clone(),
        }),
    );
    let schema = schema_builder(database)
        .data(AuthSubject::new("user-1"))
        .data(enforcement)
        .finish();

    let denied = schema
        .execute(
            r#"mutation {
                createAssuranceRecord(input: { value: "blocked" }) { success }
            }"#,
        )
        .await;
    assert_eq!(denied.errors.len(), 1, "{:?}", denied.errors);
    assert_eq!(
        denied.errors[0]
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(ToString::to_string),
        Some("\"STEP_UP_REQUIRED\"".to_string())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let exempt = schema.execute("mutation { serviceHeartbeat }").await;
    assert!(exempt.errors.is_empty(), "{:?}", exempt.errors);
    assert_eq!(exempt.data.into_json()?["serviceHeartbeat"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn manifest_is_advisory_complete_and_deterministic() {
    let registry = registry();
    registry.audit().assert_complete();
    let first = registry.manifest().to_json().unwrap();
    let second = registry.manifest().to_json().unwrap();
    assert_eq!(first, second);
    assert!(first.contains("createAssuranceRecord"));
    assert!(first.contains("interactive.recent-auth"));
    assert!(first.contains("custom:service-heartbeat:v1"));
    assert!(first.contains("service operation uses a non-interactive principal"));
}
