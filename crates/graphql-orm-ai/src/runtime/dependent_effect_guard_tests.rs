//! Runtime wiring of the dependent-effect guard for generated deletes.
//!
//! These tests drive the AI runtime's prepared automatic and approved
//! mutation paths into a fake authenticated executor. The executor runs the
//! same parent/child/leaf/set-null graph as the ORM's
//! `dependent_delete_repro` fixture through a real generated schema, so the
//! ORM's transaction-bound planner decides every guarded delete.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agql_auth::{
    AccessTokenMetadata, AuthPrincipal, AuthUser, CurrentPrincipalResolver, PrincipalReference,
    ResolvedPrincipal, SessionContext,
};
use async_trait::async_trait;
use graphql_orm::graphql::orm::{ApplyOptions, DependentDeleteGuard, OrmSchemaModule};
use graphql_orm::prelude::{Database, SqliteBackend};
use graphql_orm_operation_catalog::{
    GeneratedGraphqlOperationCategory::{Delete, Update},
    GraphqlOperationCatalog, GraphqlSemanticCatalog,
};
use serde_json::json;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::*;
use crate::dependent_effects::tests::registered_capabilities::{entity, generated};
use crate::{
    AiAccessDecision, AiAccessPolicy, AiApprovalGrant, AiApprovalId, AiApprovalState,
    AiContentProtectionMode, AiContentProtectionPolicy, AiContentProtectionPolicyResolver,
    AiDependentEffect, AiDependentEffectPolicy, AiDeploymentEgressBoundary, AiDestinationTrust,
    AiEgressCapability, AiEgressDecision, AiEgressManifest, AiEgressPolicy,
    AiGeneratedGraphqlTargetPolicyBinding, AiGraphqlMutationCapabilityCatalog,
    AiGraphqlQueryCapabilityLimits, AiRunId, AiScope, AiSessionAction, AiSessionId,
    AiToolAuthorizationDecision, AiToolAuthorizationPolicy, AiToolCallId,
    AuthenticatedGraphqlExecutor, DataClassification, DatabaseManagedContentProtector,
    EnvironmentSecretStore, GraphqlExecutionTarget, GraphqlExecutionTargetClass,
    GraphqlExecutionTargetId, GraphqlExecutionTargetRegistry, GraphqlRequestContext,
    GraphqlRequestContextFactory, ToolExecutionError, ToolGraphqlResponse,
};

/// The ORM side: the dependent-delete fixture graph with a generated schema.
mod orm {
    use graphql_orm::graphql::orm::{ApplyOptions, Entity};
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        dependent_deletes,
        table = "delete_parents",
        plural = "DeleteParents",
        default_sort = "id ASC"
    )]
    pub struct DeleteParent {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        pub id: String,
        #[sortable]
        #[filterable(type = "string")]
        pub name: String,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        dependent_deletes,
        table = "delete_children",
        plural = "DeleteChildren",
        default_sort = "id ASC"
    )]
    pub struct DeleteChild {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        pub id: String,
        #[sortable]
        #[filterable(type = "string")]
        pub parent_id: String,
        #[relation(
            target = "DeleteParent",
            from = "parent_id",
            to = "id",
            on_delete = "cascade"
        )]
        pub parent: Option<DeleteParent>,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        dependent_deletes,
        table = "delete_leaves",
        plural = "DeleteLeaves",
        default_sort = "id ASC"
    )]
    pub struct DeleteLeaf {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        pub id: String,
        #[sortable]
        #[filterable(type = "string")]
        pub child_id: String,
        #[relation(
            target = "DeleteChild",
            from = "child_id",
            to = "id",
            on_delete = "cascade"
        )]
        pub child: Option<DeleteChild>,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        dependent_deletes,
        table = "delete_nulls",
        plural = "DeleteNulls",
        default_sort = "id ASC"
    )]
    pub struct DeleteNull {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        pub id: String,
        #[sortable]
        #[filterable(type = "string")]
        pub parent_id: Option<String>,
        #[relation(
            target = "DeleteParent",
            from = "parent_id",
            to = "id",
            on_delete = "set_null"
        )]
        pub parent: Option<DeleteParent>,
    }

    schema_roots! {
        entities: [DeleteParent, DeleteChild, DeleteLeaf, DeleteNull],
    }

    pub type Schema = async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

    /// One parent with a cascading child, a cascading grandchild and a
    /// set-null dependent, the graph of the ORM's dependent-delete fixture.
    /// The schema comes from the entities' own migration, so the physical
    /// foreign keys are the ones the relations declare.
    pub async fn fixture() -> (Database, Schema) {
        let database = Database::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "ai-dependent-effect-guard-orm-v1",
                "AI dependent effect guard fixture",
                &[
                    DeleteParent::metadata(),
                    DeleteChild::metadata(),
                    DeleteLeaf::metadata(),
                    DeleteNull::metadata(),
                ],
            )
            .await
            .expect("fixture schema plans");
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .expect("fixture schema applies");
        DeleteParent::insert(
            &database,
            CreateDeleteParentInput {
                id: "parent".to_owned(),
                name: "parent".to_owned(),
            },
        )
        .await
        .expect("parent row");
        DeleteChild::insert(
            &database,
            CreateDeleteChildInput {
                id: "child".to_owned(),
                parent_id: "parent".to_owned(),
            },
        )
        .await
        .expect("child row");
        DeleteLeaf::insert(
            &database,
            CreateDeleteLeafInput {
                id: "leaf".to_owned(),
                child_id: "child".to_owned(),
            },
        )
        .await
        .expect("leaf row");
        DeleteNull::insert(
            &database,
            CreateDeleteNullInput {
                id: "null".to_owned(),
                parent_id: Some("parent".to_owned()),
            },
        )
        .await
        .expect("set-null row");
        let registry = StaticDependentDeletes::<DefaultBackend>::new(
            RuntimeDependentLimits::new(10, 10).expect("limits"),
        )
        .register::<DeleteParent>()
        .expect("parent")
        .register::<DeleteChild>()
        .expect("child")
        .register::<DeleteLeaf>()
        .expect("leaf")
        .register::<DeleteNull>()
        .expect("set-null dependent");
        let schema = schema_builder(database.clone())
            .data(AuthSubject::new("test-principal"))
            .data(registry)
            .finish();
        (database, schema)
    }

    /// Rows left in each fixture table, and whether the set-null dependent
    /// still references the parent.
    pub async fn remaining(database: &Database) -> ([i64; 4], bool) {
        let count = |result: graphql_orm::Result<i64>| result.expect("count");
        let counts = [
            count(DeleteParent::query(database.pool()).count().await),
            count(DeleteChild::query(database.pool()).count().await),
            count(DeleteLeaf::query(database.pool()).count().await),
            count(DeleteNull::query(database.pool()).count().await),
        ];
        let referenced = DeleteNull::find_by_id(database, &"null".to_owned())
            .await
            .expect("set-null read")
            .is_some_and(|row| row.parent_id.as_deref() == Some("parent"));
        (counts, referenced)
    }
}

const INTACT: ([i64; 4], bool) = ([1, 1, 1, 1], true);
const DELETED: ([i64; 4], bool) = ([0, 0, 0, 1], false);

const SDL: &str = r#"
    schema { query: Query, mutation: Mutation }
    type Query { Probe: String! }
    type Mutation {
      DeleteDeleteParent(id: ID!): DeleteParent
      DeleteDeleteChild(id: ID!): DeleteChild
      DeleteDeleteLeaf(id: ID!): DeleteLeaf
      DeleteDeleteNull(id: ID!): DeleteNull
      UpdateDeleteNull(id: ID!): DeleteNull
    }
    type DeleteParent { id: String!, name: String! }
    type DeleteChild { id: String!, parentId: String! }
    type DeleteLeaf { id: String!, childId: String! }
    type DeleteNull { id: String!, parentId: String! }
"#;

const ORM_DELETE: &str = r#"mutation { deleteDeleteParent(id: "parent") { success } }"#;

fn target() -> GraphqlExecutionTargetId {
    GraphqlExecutionTargetId::parse("records.graphql").expect("target")
}

/// How the fake host executor treats the guard it is handed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Host {
    /// Attaches the guard to the ORM request, as the contract requires.
    Attaches,
    /// Implements the guarded method but runs the delete without the guard.
    SkipsGuard,
    /// Reports success without running anything, so no receipt exists.
    NoReceipt,
}

struct OrmExecutor {
    schema: orm::Schema,
    host: Host,
    dispatched: Arc<AtomicUsize>,
}

impl OrmExecutor {
    async fn run(&self, guard: Option<DependentDeleteGuard>) -> Result<(), ToolExecutionError> {
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        let mut request = async_graphql::Request::new(ORM_DELETE);
        if let Some(guard) = guard {
            request = request.data(guard);
        }
        let response = self.schema.execute(request).await;
        if response.errors.is_empty() {
            Ok(())
        } else {
            Err(ToolExecutionError::Execution)
        }
    }
}

fn deleted_parent() -> ToolGraphqlResponse {
    ToolGraphqlResponse {
        data: json!({"DeleteDeleteParent": {"id": "parent"}}),
        error_codes: Vec::new(),
        application_audit_ref: None,
    }
}

#[async_trait]
impl AuthenticatedGraphqlExecutor for OrmExecutor {
    async fn execute_registered_with_effect_guard(
        &self,
        _context: GraphqlRequestContext,
        _binding: &AiRegisteredToolExecutionBinding,
        _request: ToolGraphqlRequest,
        guard: DependentDeleteGuard,
    ) -> Result<ToolGraphqlResponse, ToolExecutionError> {
        match self.host {
            Host::Attaches => self.run(Some(guard)).await?,
            Host::SkipsGuard => self.run(None).await?,
            Host::NoReceipt => {
                self.dispatched.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(deleted_parent())
    }

    async fn execute(
        &self,
        _context: GraphqlRequestContext,
        _request: ToolGraphqlRequest,
    ) -> Result<ToolGraphqlResponse, ToolExecutionError> {
        self.run(None).await?;
        Ok(deleted_parent())
    }
}

/// An executor written before the guarded method existed: it keeps the
/// trait's refusing default and would delete unguarded if it were called.
struct LegacyExecutor {
    schema: orm::Schema,
    dispatched: Arc<AtomicUsize>,
}

#[async_trait]
impl AuthenticatedGraphqlExecutor for LegacyExecutor {
    async fn execute(
        &self,
        _context: GraphqlRequestContext,
        _request: ToolGraphqlRequest,
    ) -> Result<ToolGraphqlResponse, ToolExecutionError> {
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        let response = self.schema.execute(ORM_DELETE).await;
        if !response.errors.is_empty() {
            return Err(ToolExecutionError::Execution);
        }
        Ok(deleted_parent())
    }
}

fn principal() -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: "user-1".to_owned(),
        session_id: Uuid::from_u128(1),
        roles: vec![],
        scopes: vec!["records:write".to_owned()],
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata::default(),
    })
}

struct Resolver;

#[async_trait]
impl CurrentPrincipalResolver for Resolver {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        ResolvedPrincipal::new(reference.clone(), principal(), OffsetDateTime::UNIX_EPOCH)
    }
}

struct ContextFactory;

#[async_trait]
impl GraphqlRequestContextFactory for ContextFactory {
    async fn build(
        &self,
        principal: &ResolvedPrincipal,
        _target: &GraphqlExecutionTarget,
        _request: &ToolGraphqlRequest,
    ) -> Result<GraphqlRequestContext, ToolExecutionError> {
        Ok(GraphqlRequestContext::new(
            principal.principal().subject().to_owned(),
        ))
    }
}

struct Allow;

#[async_trait]
impl AiAccessPolicy for Allow {
    async fn can_access_scope(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        AiAccessDecision::allow("test", "access-v1")
    }

    async fn can_access_session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        AiAccessDecision::allow("test", "access-v1")
    }
}

#[async_trait]
impl AiToolAuthorizationPolicy for Allow {
    async fn authorize(
        &self,
        principal: &ResolvedPrincipal,
        _scope: &AiScope,
        _descriptor: &AiToolDescriptor,
        _variables: &serde_json::Value,
    ) -> AiToolAuthorizationDecision {
        AiToolAuthorizationDecision::allow(
            "test",
            "tool-policy-v1",
            format!("auth-state:{}", principal.principal().subject()),
        )
    }
}

#[async_trait]
impl AiEgressPolicy for Allow {
    async fn authorize(
        &self,
        principal: &ResolvedPrincipal,
        manifest: &AiEgressManifest,
    ) -> AiEgressDecision {
        AiEgressDecision::allow(manifest, "egress-v1", principal.principal().subject())
    }
}

#[async_trait]
impl AiContentProtectionPolicyResolver for Allow {
    async fn resolve(
        &self,
        _principal: &AuthPrincipal,
        scope: &AiScope,
    ) -> Result<AiContentProtectionPolicy, AiError> {
        Ok(AiContentProtectionPolicy {
            scope: scope.clone(),
            mode: AiContentProtectionMode::DatabaseManaged,
            key_policy_reference: None,
            version: 1,
            ready: true,
        })
    }
}

/// Registered generated mutations with the root delete classified `root`.
/// Every dependent operation is automatic, so the AI policy admits a
/// correct declaration for either root classification.
fn mutations(
    root: AiMutationExecutionPolicy,
) -> (GraphqlSemanticCatalog, AiGraphqlMutationCapabilityCatalog) {
    use AiMutationExecutionPolicy::Automatic;
    let semantics = GraphqlSemanticCatalog::compose_with_custom(
        [
            entity("DeleteParent", "name"),
            entity("DeleteChild", "parentId"),
            entity("DeleteLeaf", "childId"),
            entity("DeleteNull", "parentId"),
        ],
        &GraphqlOperationCatalog::compose(std::iter::empty()),
        [
            generated("DeleteDeleteParent", "DeleteParent", Delete, root),
            generated("DeleteDeleteChild", "DeleteChild", Delete, Automatic),
            generated("DeleteDeleteLeaf", "DeleteLeaf", Delete, Automatic),
            generated("DeleteDeleteNull", "DeleteNull", Delete, Automatic),
            generated("UpdateDeleteNull", "DeleteNull", Update, Automatic),
        ],
    )
    .expect("semantic catalogue");
    let catalog = AiGraphqlMutationCapabilityCatalog::compile(
        "records",
        target(),
        SDL,
        &semantics,
        AiGraphqlQueryCapabilityLimits::default(),
    )
    .expect("mutation capabilities");
    (semantics, catalog)
}

struct Fixture {
    runtime: AiRuntime,
    orm_database: Database<SqliteBackend>,
    dispatched: Arc<AtomicUsize>,
    capability_id: AiToolId,
    capability_fingerprint: String,
    update_id: AiToolId,
    update_fingerprint: String,
    _store: Database<SqliteBackend>,
}

/// Executors the fixture can install.
enum Executor {
    Orm(Host),
    Legacy,
}

async fn fixture(
    root: AiMutationExecutionPolicy,
    policy: Option<AiDependentEffectPolicy>,
    executor: Executor,
) -> Fixture {
    let (semantics, mutations) = mutations(root);
    let capability = |field: &str| {
        let capability = mutations
            .capabilities()
            .find(|capability| capability.field_name() == field)
            .expect("capability");
        (capability.id().clone(), capability.fingerprint().to_owned())
    };
    let (capability_id, capability_fingerprint) = capability("DeleteDeleteParent");
    let (update_id, update_fingerprint) = capability("UpdateDeleteNull");
    let mut target_policy = AiGeneratedGraphqlTargetPolicySet::new();
    target_policy
        .bind(
            AiGeneratedGraphqlTargetPolicyBinding::new(
                target(),
                mutations.finished_schema_fingerprint(),
                semantics.fingerprint.clone(),
            )
            .expect("target binding")
            .allow_automatic_mutations()
            .allow_approval_required_mutations(),
        )
        .expect("bound target");
    let mut tool_catalog = AiToolCatalog::new();
    tool_catalog
        .register_mutation_capability_catalog(&mutations)
        .expect("registered mutations");
    let mut targets = GraphqlExecutionTargetRegistry::new();
    targets
        .register(GraphqlExecutionTarget {
            id: target(),
            class: GraphqlExecutionTargetClass::Local,
            audience: None,
            resource_type: None,
            resource_id: None,
            schema_fingerprint: mutations.finished_schema_fingerprint().to_owned(),
        })
        .expect("target registers");

    let (orm_database, schema) = orm::fixture().await;
    let dispatched = Arc::new(AtomicUsize::new(0));
    let executor: Arc<dyn AuthenticatedGraphqlExecutor> = match executor {
        Executor::Orm(host) => Arc::new(OrmExecutor {
            schema,
            host,
            dispatched: dispatched.clone(),
        }),
        Executor::Legacy => Arc::new(LegacyExecutor {
            schema,
            dispatched: dispatched.clone(),
        }),
    };
    let mut builder = AiRuntime::builder()
        .principal_resolver(Arc::new(Resolver))
        .access_policy(Arc::new(Allow))
        .tool_authorization_policy(Arc::new(Allow))
        .request_context_factory(Arc::new(ContextFactory))
        .graphql_executor(executor)
        .graphql_targets(targets)
        .egress_policy(Arc::new(Allow))
        .deployment_egress(AiDeploymentEgressBoundary {
            allowed_destination_trust: [AiDestinationTrust::ManagedProvider].into(),
            allowed_capabilities: [AiEgressCapability::ModelInference].into(),
            maximum_classification: DataClassification::Internal,
            maximum_bytes: 1_000,
            maximum_attachments: 0,
        })
        .maximum_tool_maturity(ToolMaturity::AutonomousWrite)
        .generated_graphql_target_policy(target_policy)
        .tool_catalog(tool_catalog)
        .secret_store(Arc::new(EnvironmentSecretStore::new()))
        .content_protection_policy_resolver(Arc::new(Allow))
        .content_protector(Arc::new(DatabaseManagedContentProtector));
    if let Some(policy) = policy {
        builder = builder.dependent_effect_policy(policy);
    }
    let runtime = builder.build().expect("runtime builds");

    let store = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("AI store opens");
    let plan = store
        .schema()
        .plan_migration_to_entities(
            "ai-dependent-effect-guard-v1",
            "AI dependent effect guard test",
            crate::AiSchemaModule.entities(),
        )
        .await
        .expect("AI schema plans");
    store
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("AI schema applies");
    runtime
        .start_gate()
        .open(&crate::orm_runtime_recovery::open_test_epoch(&store).await)
        .expect("runtime opens");
    Fixture {
        runtime,
        orm_database,
        dispatched,
        capability_id,
        capability_fingerprint,
        update_id,
        update_fingerprint,
        _store: store,
    }
}

fn invocation() -> GraphqlInvocationContext {
    GraphqlInvocationContext {
        run_id: AiRunId::new(),
        tool_call_id: AiToolCallId::new(),
        scope: AiScope::new("project", "project-1"),
        correlation_id: "dependent-effect-guard".to_owned(),
        causation_id: "dependent-effect-guard".to_owned(),
        delegation_reference: None,
        idempotency_key: None,
    }
}

fn delete_plan() -> serde_json::Value {
    json!({"arguments": {"id": "parent"}, "fields": {"id": true}, "relationships": {}})
}

impl Fixture {
    fn prepare(&self) -> Result<AiPreparedGraphqlMutation, AiError> {
        self.runtime.prepare_mutation_capability(
            &self.capability_id,
            &self.capability_fingerprint,
            delete_plan(),
            invocation(),
        )
    }

    async fn automatic(&self) -> Result<AiToolExecutionResult, AiError> {
        let prepared = self.prepare()?;
        self.runtime
            .execute_prepared_automatic_mutation(&principal().reference(), prepared)
            .await
    }

    async fn approved(&self) -> Result<AiToolExecutionResult, AiError> {
        let reference = principal().reference();
        let prepared = self.prepare()?;
        let authority = self
            .runtime
            .preauthorize_prepared_mutation(&reference, &prepared)
            .await?;
        let binding = AiApprovalBinding {
            tool_call_id: prepared.request.invocation.tool_call_id,
            session_id: AiSessionId(Uuid::new_v4()),
            scope: prepared.request.invocation.scope.clone(),
            tool_fingerprint: prepared.capability_fingerprint.clone(),
            argument_hash: crate::tools::canonical_json_digest(&prepared.request.variables),
            operation: prepared
                .descriptor
                .graphql_contract
                .clone()
                .expect("generated contract"),
            principal_reference_fingerprint: AiApprovalBinding::principal_fingerprint(&reference),
            delegated_actor_subject: None,
            delegation_reference: None,
            policy_version: authority.policy_version.clone(),
            authorization_state_digest: authority.authorization_state_digest.clone(),
            resources: Vec::new(),
            preview_hash: "6".repeat(64),
        };
        let now = OffsetDateTime::now_utc();
        let grant = AiApprovalGrant {
            id: AiApprovalId::new(),
            binding_hash: binding.stable_hash(),
            approver_subject: "approver".to_owned(),
            state: AiApprovalState::Approved,
            approved_at: now - Duration::seconds(1),
            expires_at: now + Duration::minutes(5),
        };
        let approval = ConsumedAiApproval::new(
            grant.authorize(&binding, now).expect("approval authorizes"),
            now,
        );
        self.runtime
            .execute_approved_prepared_mutation(&reference, prepared, &approval, &binding)
            .await
    }

    async fn run(&self, root: AiMutationExecutionPolicy) -> Result<AiToolExecutionResult, AiError> {
        match root {
            AiMutationExecutionPolicy::Automatic => self.automatic().await,
            _ => self.approved().await,
        }
    }

    async fn remaining(&self) -> ([i64; 4], bool) {
        orm::remaining(&self.orm_database).await
    }

    fn dispatched(&self) -> usize {
        self.dispatched.load(Ordering::SeqCst)
    }
}

fn declared(effects: impl IntoIterator<Item = AiDependentEffect>) -> AiDependentEffectPolicy {
    AiDependentEffectPolicy::new()
        .declare_delete(target(), "DeleteParent", effects)
        .expect("declaration")
}

fn correct_effects() -> Vec<AiDependentEffect> {
    vec![
        AiDependentEffect::delete("DeleteChild")
            .expect("effect")
            .with_maximum_rows(1),
        AiDependentEffect::delete("DeleteLeaf")
            .expect("effect")
            .with_maximum_rows(1),
        AiDependentEffect::set_null("DeleteNull", "parentId")
            .expect("effect")
            .with_maximum_rows(1),
    ]
}

const ROOTS: [AiMutationExecutionPolicy; 2] = [
    AiMutationExecutionPolicy::Automatic,
    AiMutationExecutionPolicy::ApprovalRequired,
];

/// The ORM planner decides every declaration the AI policy admits, on both
/// the prepared automatic and the approved path.
#[tokio::test]
async fn guarded_deletes_follow_the_orm_plan_on_both_paths() {
    for root in ROOTS {
        // A root with no declaration never reaches the executor.
        let undeclared_root = fixture(
            root,
            Some(AiDependentEffectPolicy::new()),
            Executor::Orm(Host::Attaches),
        )
        .await;
        assert!(
            matches!(undeclared_root.run(root).await, Err(AiError::Forbidden)),
            "{root:?}"
        );
        assert_eq!(undeclared_root.dispatched(), 0, "{root:?}");
        assert_eq!(undeclared_root.remaining().await, INTACT, "{root:?}");

        let mut wrong = correct_effects();
        wrong[2] = AiDependentEffect::delete("DeleteNull")
            .expect("effect")
            .with_maximum_rows(1);
        let mut bounded = correct_effects();
        bounded[0] = AiDependentEffect::delete("DeleteChild")
            .expect("effect")
            .with_maximum_rows(0);
        for (label, effects) in [
            ("undeclared effects", Vec::new()),
            ("wrong declaration", wrong),
            ("row bound exceeded", bounded),
        ] {
            let refused =
                fixture(root, Some(declared(effects)), Executor::Orm(Host::Attaches)).await;
            let result = refused.run(root).await;
            assert!(
                matches!(result, Err(AiError::ToolExecutionFailed)),
                "{root:?} {label}: {result:?}"
            );
            assert_eq!(refused.dispatched(), 1, "{root:?} {label}");
            assert_eq!(
                refused.remaining().await,
                INTACT,
                "{root:?} {label} rolls back"
            );
        }

        let accepted = fixture(
            root,
            Some(declared(correct_effects())),
            Executor::Orm(Host::Attaches),
        )
        .await;
        accepted
            .run(root)
            .await
            .unwrap_or_else(|error| panic!("{root:?} correct declaration: {error:?}"));
        assert_eq!(accepted.remaining().await, DELETED, "{root:?}");
    }
}

/// An executor that keeps the trait's default guarded method refuses
/// before anything runs.
#[tokio::test]
async fn the_default_guarded_method_refuses_before_execution() {
    for root in ROOTS {
        let fixture = fixture(root, Some(declared(correct_effects())), Executor::Legacy).await;
        assert!(
            matches!(fixture.run(root).await, Err(AiError::Forbidden)),
            "{root:?}"
        );
        assert_eq!(fixture.dispatched(), 0, "{root:?}");
        assert_eq!(fixture.remaining().await, INTACT, "{root:?}");
    }
}

/// A host that reports success without the ORM's commit receipt is refused
/// on both paths.
#[tokio::test]
async fn a_missing_commit_receipt_refuses() {
    for root in ROOTS {
        let fixture = fixture(
            root,
            Some(declared(correct_effects())),
            Executor::Orm(Host::NoReceipt),
        )
        .await;
        assert!(
            matches!(fixture.run(root).await, Err(AiError::Forbidden)),
            "{root:?}"
        );
        assert_eq!(fixture.dispatched(), 1, "{root:?}");
        assert_eq!(fixture.remaining().await, INTACT, "{root:?}");
    }
}

/// A host that implements the guarded method but does not attach the guard
/// is a host-contract breach. The ORM then runs an ordinary unguarded
/// delete, which commits; the bridge detects the missing receipt only after
/// that commit and refuses the result with `Authorization`.
#[tokio::test]
async fn a_host_that_skips_the_guard_is_detected_after_commit() {
    for root in ROOTS {
        let fixture = fixture(
            root,
            Some(declared(correct_effects())),
            Executor::Orm(Host::SkipsGuard),
        )
        .await;
        assert!(
            matches!(fixture.run(root).await, Err(AiError::Forbidden)),
            "{root:?}"
        );
        assert_eq!(fixture.dispatched(), 1, "{root:?}");
        assert_eq!(
            fixture.remaining().await,
            DELETED,
            "{root:?}: the unguarded write committed before detection"
        );
    }

    // The bridge's own refusal is `Authorization`.
    let fixture = fixture(
        AiMutationExecutionPolicy::Automatic,
        Some(declared(correct_effects())),
        Executor::Orm(Host::SkipsGuard),
    )
    .await;
    let prepared = fixture.prepare().expect("prepared");
    let guard = fixture
        .runtime
        .mutation_effect_guard(&prepared)
        .expect("guard lookup")
        .expect("a guard for the delete");
    let result = fixture
        .runtime
        .tool_bridge
        .execute_generated_mutation(
            &principal().reference(),
            &prepared.descriptor.id,
            &prepared.capability_fingerprint,
            &prepared.descriptor,
            prepared.request,
            Some(guard.clone()),
        )
        .await;
    assert!(
        matches!(result, Err(ToolExecutionError::Authorization)),
        "{result:?}"
    );
    assert!(!guard.was_committed());
}

/// With a policy installed, a delete whose capability or declaration cannot
/// be found fails closed; without a policy, or for a non-delete, no guard is
/// built.
#[tokio::test]
async fn guard_lookup_failure_is_forbidden() {
    let declared_fixture = fixture(
        AiMutationExecutionPolicy::Automatic,
        Some(declared(correct_effects())),
        Executor::Orm(Host::Attaches),
    )
    .await;
    let prepared = declared_fixture.prepare().expect("prepared");
    assert!(
        declared_fixture
            .runtime
            .mutation_effect_guard(&prepared)
            .expect("declared delete")
            .is_some()
    );

    // The capability is not in the catalogue.
    let mut unregistered = prepared.clone();
    unregistered.descriptor.id = AiToolId::parse("records.unregistered").expect("id");
    assert!(matches!(
        declared_fixture
            .runtime
            .mutation_effect_guard(&unregistered),
        Err(AiError::Forbidden)
    ));

    // The installed policy declares nothing for this root.
    let undeclared = fixture(
        AiMutationExecutionPolicy::Automatic,
        Some(AiDependentEffectPolicy::new()),
        Executor::Orm(Host::Attaches),
    )
    .await;
    assert!(matches!(
        undeclared.runtime.mutation_effect_guard(&prepared),
        Err(AiError::Forbidden)
    ));

    // No installed policy, and a mutation that is not a delete.
    let unguarded = fixture(
        AiMutationExecutionPolicy::Automatic,
        None,
        Executor::Orm(Host::Attaches),
    )
    .await;
    assert!(matches!(
        unguarded.runtime.mutation_effect_guard(&prepared),
        Ok(None)
    ));
    let update = declared_fixture
        .runtime
        .prepare_mutation_capability(
            &declared_fixture.update_id,
            &declared_fixture.update_fingerprint,
            json!({"arguments": {"id": "null"}, "fields": {"id": true}, "relationships": {}}),
            invocation(),
        )
        .expect("prepared update");
    assert!(matches!(
        declared_fixture.runtime.mutation_effect_guard(&update),
        Ok(None)
    ));
}
