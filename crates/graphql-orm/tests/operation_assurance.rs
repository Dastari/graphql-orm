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

use graphql_orm::async_graphql::{ErrorExtensions, Guard};
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

/// Counts host guard evaluations for the request.
#[derive(Default)]
struct HostGuardCalls(AtomicUsize);

struct HostGuard;

impl Guard for HostGuard {
    async fn check(
        &self,
        ctx: &graphql_orm::async_graphql::Context<'_>,
    ) -> graphql_orm::async_graphql::Result<()> {
        if let Some(calls) = ctx.data_opt::<Arc<HostGuardCalls>>() {
            calls.0.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// Described roots whose own guards would otherwise skip or misclassify
/// assurance: the attribute composes the correct check before each of them.
#[derive(Default)]
struct HostGuardedMutations;

#[graphql_orm_custom_operations(kind = "mutation")]
#[graphql_orm::async_graphql::Object]
impl HostGuardedMutations {
    /// Runs a host-guarded action.
    #[graphql(guard = "HostGuard")]
    async fn host_guarded(&self) -> bool {
        true
    }

    /// Declares assurance with the query kind on a mutation root.
    #[graphql(guard = "DeclaredAssuranceGuard::new(GraphqlOperationKind::Query).and(HostGuard)")]
    async fn wrong_kind(&self) -> bool {
        true
    }

    /// Offers assurance only as an alternative to the host guard.
    #[graphql(guard = HostGuard.or(DeclaredAssuranceGuard::new(GraphqlOperationKind::Mutation)))]
    async fn either_guard(&self) -> bool {
        true
    }
}

/// A root-level guard still applies to resolvers without their own guard, and
/// `skip = false` exposes the resolver with its assurance guard.
#[derive(Default)]
struct RootGuardedMutations;

#[graphql_orm_custom_operations(kind = "mutation")]
#[graphql_orm::async_graphql::Object(guard = "HostGuard")]
impl RootGuardedMutations {
    /// Runs under the root's host guard.
    async fn root_guarded(&self) -> bool {
        true
    }

    /// Explicitly not skipped.
    #[graphql(skip = false)]
    async fn not_skipped(&self) -> bool {
        true
    }
}

const HOST_GUARDED: [&str; 5] = [
    "hostGuarded",
    "wrongKind",
    "eitherGuard",
    "rootGuarded",
    "notSkipped",
];

schema_roots! {
    backend: "sqlite",
    entities: [AssuranceRecord],
    extra_mutation_types: [CustomAssuranceMutations],
    described_mutation_types: [
        DescribedAssuranceMutations,
        HostGuardedMutations,
        RootGuardedMutations,
    ],
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
    for (operation_id, field) in [
        ("custom:rotate-key:v1", "rotateKey"),
        ("custom:host-guarded:v1", "hostGuarded"),
        ("custom:wrong-kind:v1", "wrongKind"),
        ("custom:either-guard:v1", "eitherGuard"),
        ("custom:root-guarded:v1", "rootGuarded"),
        ("custom:not-skipped:v1", "notSkipped"),
    ] {
        builder
            .register_custom(
                operation_id,
                GraphqlOperationKind::Mutation,
                field,
                AssuranceActorClass::Interactive,
            )
            .unwrap();
    }
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
        [
            "Mutation.eitherGuard",
            "Mutation.hostGuarded",
            "Mutation.notSkipped",
            "Mutation.rootGuarded",
            "Mutation.rotateKey",
            "Mutation.wrongKind",
        ]
    );

    AssuranceEnforcement::new(Arc::new(registry()), denying(&calls))
        .finish_schema(schema_builder(database))?;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Ok(())
}

/// Admits every requirement, counting evaluations.
#[derive(Debug, Default)]
struct AdmittingEvaluator(AtomicUsize);

impl AssuranceRequirementEvaluator for AdmittingEvaluator {
    fn enforce(
        &self,
        _ctx: &graphql_orm::async_graphql::Context<'_>,
        _actor_class: AssuranceActorClass,
        _policy_id: &str,
    ) -> graphql_orm::async_graphql::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A host guard, a wrong-kind assurance guard or an `.or` alternative cannot
/// bypass the mutation coordinate's default requirement, even unregistered.
#[tokio::test]
async fn host_guards_compose_after_the_described_root_assurance_check()
-> Result<(), Box<dyn std::error::Error>> {
    let config = AssuranceSchemaConfig::legacy()
        .with_default_interactive_mutation_policy("interactive.recent-auth")
        .unwrap();
    let registry = Arc::new(
        OperationAssuranceRegistry::builder(graphql_orm_operation_catalog(), config)
            .build()
            .unwrap(),
    );

    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let host = Arc::new(HostGuardCalls::default());
    let denied = schema_builder(database.clone())
        .data(AuthSubject::new("user-1"))
        .data(host.clone())
        .data(AssuranceEnforcement::new(registry.clone(), denying(&calls)))
        .finish();
    for field in HOST_GUARDED {
        let response = denied.execute(format!("mutation {{ {field} }}")).await;
        assert!(step_up(&response), "{field}: {:?}", response.errors);
    }
    assert_eq!(calls.load(Ordering::SeqCst), HOST_GUARDED.len());
    assert_eq!(host.0.load(Ordering::SeqCst), 0);

    let admitting = Arc::new(AdmittingEvaluator::default());
    let admitted = schema_builder(database)
        .data(AuthSubject::new("user-1"))
        .data(host.clone())
        .data(AssuranceEnforcement::new(registry, admitting.clone()))
        .finish();
    for field in HOST_GUARDED {
        let response = admitted.execute(format!("mutation {{ {field} }}")).await;
        assert!(response.errors.is_empty(), "{field}: {:?}", response.errors);
        assert_eq!(response.data.into_json()?[field], true);
    }
    // Each host guard still runs once the assurance check admits the caller.
    assert_eq!(host.0.load(Ordering::SeqCst), HOST_GUARDED.len());
    assert!(admitting.0.load(Ordering::SeqCst) >= HOST_GUARDED.len());
    Ok(())
}
