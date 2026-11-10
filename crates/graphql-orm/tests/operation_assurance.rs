// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
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

/// A described custom root: the attribute installs its assurance guard.
#[derive(Default)]
struct DescribedAssuranceMutations;

#[graphql_orm_custom_operations(kind = "mutation")]
#[graphql_orm::async_graphql::Object]
impl DescribedAssuranceMutations {
    /// Rotate the signing key.
    async fn rotate_key(&self) -> bool {
        true
    }
}

schema_roots! {
    backend: "sqlite",
    entities: [AssuranceRecord],
    extra_mutation_types: [CustomAssuranceMutations],
    described_mutation_types: [DescribedAssuranceMutations],
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

fn strict_builder() -> OperationAssuranceRegistryBuilder {
    let config = AssuranceSchemaConfig::legacy()
        .with_default_interactive_mutation_policy("interactive.recent-auth")
        .unwrap()
        .with_strict_mutation_classification(true);
    OperationAssuranceRegistry::builder(graphql_orm_operation_catalog(), config)
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
    builder
        .register_custom(
            "custom:rotate-key:v1",
            GraphqlOperationKind::Mutation,
            "rotateKey",
            AssuranceActorClass::Interactive,
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

fn denying(calls: &Arc<AtomicUsize>) -> Arc<DenyingEvaluator> {
    Arc::new(DenyingEvaluator {
        calls: calls.clone(),
    })
}

fn step_up(response: &graphql_orm::async_graphql::Response) -> bool {
    response.errors.len() == 1
        && response.errors[0]
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(ToString::to_string)
            .as_deref()
            == Some("\"STEP_UP_REQUIRED\"")
}

/// A custom mutation missing from a non-strict registry receives the
/// configured interactive default.
#[tokio::test]
async fn unregistered_custom_mutations_apply_the_default_interactive_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let config = AssuranceSchemaConfig::legacy()
        .with_default_interactive_mutation_policy("interactive.recent-auth")
        .unwrap();
    let registry = OperationAssuranceRegistry::builder(graphql_orm_operation_catalog(), config)
        .build()
        .unwrap();
    let schema = schema_builder(database)
        .data(AuthSubject::new("user-1"))
        .data(AssuranceEnforcement::new(
            Arc::new(registry),
            denying(&calls),
        ))
        .finish();

    for mutation in ["mutation { serviceHeartbeat }", "mutation { rotateKey }"] {
        let response = schema.execute(mutation).await;
        assert!(step_up(&response), "{mutation}: {:?}", response.errors);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn described_custom_roots_enforce_their_registered_classification()
-> Result<(), Box<dyn std::error::Error>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let schema = AssuranceEnforcement::new(Arc::new(registry()), denying(&calls))
        .finish_schema(schema_builder(database).data(AuthSubject::new("user-1")))?;
    let response = schema.execute("mutation { rotateKey }").await;
    assert!(step_up(&response), "{:?}", response.errors);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn strict_schema_build_refuses_unregistered_mutation_roots()
-> Result<(), Box<dyn std::error::Error>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    // Only the generated catalogue: both custom roots are unregistered.
    let mut builder = strict_builder();
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
    let incomplete = builder.build().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let error = AssuranceEnforcement::new(Arc::new(incomplete), denying(&calls))
        .finish_schema(schema_builder(database.clone()))
        .err()
        .expect("an unregistered mutation root must fail the strict schema build");
    let AssuranceRegistryError::UnregisteredMutations(missing) = error else {
        panic!("unexpected error: {error}");
    };
    assert_eq!(
        missing.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["Mutation.rotateKey"]
    );

    AssuranceEnforcement::new(Arc::new(registry()), denying(&calls))
        .finish_schema(schema_builder(database))?;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Ok(())
}
