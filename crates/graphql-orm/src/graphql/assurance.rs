//! Provider-neutral GraphQL operation assurance declarations and manifests.
//!
//! This module owns schema classification and enforcement wiring. It does not
//! verify authentication evidence or define session policy. The optional
//! `auth-agql` bridge supplies an evaluator; without one, existing schemas keep
//! their historical behavior unless they deliberately install enforcement.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use async_graphql::Guard;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::graphql::orm::{GraphqlOperationCatalog, GraphqlOperationKind};

/// Version of the deterministic operation-assurance manifest format.
pub const OPERATION_ASSURANCE_MANIFEST_VERSION: u32 = 1;
/// Provider-neutral directive definitions represented by schema metadata.
pub const ASSURANCE_DIRECTIVE_DEFINITIONS: &str = r#"enum AssuranceActorClass {
  INTERACTIVE
  MACHINE
  SERVICE
  SAFETY_TEARDOWN
}

directive @requiresAssurance(policy: String!, actor: AssuranceActorClass!) on FIELD_DEFINITION
directive @assuranceExempt(reason: String!, actor: AssuranceActorClass!) on FIELD_DEFINITION"#;

const MAX_POLICY_ID_LENGTH: usize = 128;
const MAX_METADATA_VALUE_LENGTH: usize = 256;

/// Actor classification used by assurance defaults and client manifests.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AssuranceActorClass {
    /// A human-controlled interactive operation.
    Interactive,
    /// A non-human machine principal operation.
    Machine,
    /// A service-to-service operation.
    Service,
    /// A logout, revocation, recovery, or equivalent safety teardown operation.
    SafetyTeardown,
}

impl AssuranceActorClass {
    /// Stable lowercase manifest value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Machine => "machine",
            Self::Service => "service",
            Self::SafetyTeardown => "safety_teardown",
        }
    }

    const fn directive_value(self) -> &'static str {
        match self {
            Self::Interactive => "INTERACTIVE",
            Self::Machine => "MACHINE",
            Self::Service => "SERVICE",
            Self::SafetyTeardown => "SAFETY_TEARDOWN",
        }
    }
}

/// Whether an operation requires assurance, is exempt, or is not classified.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "classification", rename_all = "snake_case")]
pub enum OperationAssuranceClassification {
    /// No requirement or exemption was declared.
    Unclassified,
    /// The operation requires the stable policy ID.
    Required {
        /// Stable provider-neutral assurance policy ID.
        policy_id: String,
    },
    /// The operation is explicitly exempt for the documented reason.
    Exempt {
        /// Stable, non-secret explanation included in audit output and manifests.
        reason: String,
    },
}

/// Source of an operation identity in the registry.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationAssuranceOrigin {
    /// A resolver described by `GraphqlOperationCatalog`.
    Generated,
    /// A host-authored custom resolver field.
    Custom,
}

/// Stable top-level schema field coordinate.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OperationAssuranceCoordinate {
    /// GraphQL operation root.
    pub kind: GraphqlOperationKind,
    /// Exact schema field name, not an alias or document operation name.
    pub field_name: String,
}

impl OperationAssuranceCoordinate {
    /// Creates a root field coordinate.
    pub fn new(kind: GraphqlOperationKind, field_name: impl Into<String>) -> Self {
        Self {
            kind,
            field_name: field_name.into(),
        }
    }

    /// Conventional `Root.field` representation.
    pub fn field_coordinate(&self) -> String {
        format!("{}.{}", self.kind.root_type(), self.field_name)
    }
}

impl fmt::Display for OperationAssuranceCoordinate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.kind.root_type(), self.field_name)
    }
}

/// Backward-compatible schema-level assurance defaults.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AssuranceSchemaConfig {
    default_interactive_mutation_policy: Option<String>,
    strict_mutation_classification: bool,
}

impl AssuranceSchemaConfig {
    /// Creates a legacy-compatible configuration with no assurance default.
    pub const fn legacy() -> Self {
        Self {
            default_interactive_mutation_policy: None,
            strict_mutation_classification: false,
        }
    }

    /// Configures the default applied only to interactive mutations.
    pub fn with_default_interactive_mutation_policy(
        mut self,
        policy_id: impl Into<String>,
    ) -> Result<Self, AssuranceRegistryError> {
        let policy_id = policy_id.into();
        validate_policy_id(&policy_id)?;
        self.default_interactive_mutation_policy = Some(policy_id);
        Ok(self)
    }

    /// Enables or disables build-time completeness enforcement.
    pub const fn with_strict_mutation_classification(mut self, strict: bool) -> Self {
        self.strict_mutation_classification = strict;
        self
    }

    /// Returns the configured interactive mutation default.
    pub fn default_interactive_mutation_policy(&self) -> Option<&str> {
        self.default_interactive_mutation_policy.as_deref()
    }

    /// Returns whether building an incomplete registry fails.
    pub const fn strict_mutation_classification(&self) -> bool {
        self.strict_mutation_classification
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegisteredOperation {
    operation_id: String,
    coordinate: OperationAssuranceCoordinate,
    origin: OperationAssuranceOrigin,
    actor_class: AssuranceActorClass,
    classification: OperationAssuranceClassification,
}

/// Mutable builder for generated and custom operation classification.
#[derive(Clone, Debug)]
pub struct OperationAssuranceRegistryBuilder {
    config: AssuranceSchemaConfig,
    operations: BTreeMap<OperationAssuranceCoordinate, RegisteredOperation>,
}

impl OperationAssuranceRegistryBuilder {
    /// Starts with every exposed operation from a generated schema catalog.
    pub fn from_catalog(catalog: &GraphqlOperationCatalog, config: AssuranceSchemaConfig) -> Self {
        let operations = catalog
            .exposed_operations()
            .map(|operation| {
                let coordinate =
                    OperationAssuranceCoordinate::new(operation.kind(), operation.field_name());
                let registered = RegisteredOperation {
                    operation_id: operation.fingerprint().to_string(),
                    coordinate: coordinate.clone(),
                    origin: OperationAssuranceOrigin::Generated,
                    actor_class: AssuranceActorClass::Interactive,
                    classification: OperationAssuranceClassification::Unclassified,
                };
                (coordinate, registered)
            })
            .collect();
        Self { config, operations }
    }

    /// Registers one custom resolver field with a stable host-authored identity.
    pub fn register_custom(
        &mut self,
        operation_id: impl Into<String>,
        kind: GraphqlOperationKind,
        field_name: impl Into<String>,
        actor_class: AssuranceActorClass,
    ) -> Result<&mut Self, AssuranceRegistryError> {
        let operation_id = operation_id.into();
        let coordinate = OperationAssuranceCoordinate::new(kind, field_name);
        validate_metadata_value("operation_id", &operation_id)?;
        validate_field_name(&coordinate.field_name)?;
        if self.operations.contains_key(&coordinate) {
            return Err(AssuranceRegistryError::DuplicateOperation(coordinate));
        }
        self.operations.insert(
            coordinate.clone(),
            RegisteredOperation {
                operation_id,
                coordinate,
                origin: OperationAssuranceOrigin::Custom,
                actor_class,
                classification: OperationAssuranceClassification::Unclassified,
            },
        );
        Ok(self)
    }

    /// Explicitly classifies the actor for a generated or custom field.
    pub fn set_actor_class(
        &mut self,
        kind: GraphqlOperationKind,
        field_name: &str,
        actor_class: AssuranceActorClass,
    ) -> Result<&mut Self, AssuranceRegistryError> {
        self.operation_mut(kind, field_name)?.actor_class = actor_class;
        Ok(self)
    }

    /// Declares an assurance requirement for a generated or custom field.
    pub fn require(
        &mut self,
        kind: GraphqlOperationKind,
        field_name: &str,
        policy_id: impl Into<String>,
    ) -> Result<&mut Self, AssuranceRegistryError> {
        let policy_id = policy_id.into();
        validate_policy_id(&policy_id)?;
        self.operation_mut(kind, field_name)?.classification =
            OperationAssuranceClassification::Required { policy_id };
        Ok(self)
    }

    /// Declares an explicit, documented exemption for a generated or custom field.
    pub fn exempt(
        &mut self,
        kind: GraphqlOperationKind,
        field_name: &str,
        reason: impl Into<String>,
    ) -> Result<&mut Self, AssuranceRegistryError> {
        let reason = reason.into();
        validate_metadata_value("exemption reason", &reason)?;
        self.operation_mut(kind, field_name)?.classification =
            OperationAssuranceClassification::Exempt { reason };
        Ok(self)
    }

    /// Resolves defaults, audits completeness, and builds the immutable registry.
    pub fn build(mut self) -> Result<OperationAssuranceRegistry, AssuranceRegistryError> {
        if let Some(policy_id) = self.config.default_interactive_mutation_policy.clone() {
            for operation in self.operations.values_mut() {
                if operation.coordinate.kind == GraphqlOperationKind::Mutation
                    && operation.actor_class == AssuranceActorClass::Interactive
                    && operation.classification == OperationAssuranceClassification::Unclassified
                {
                    operation.classification = OperationAssuranceClassification::Required {
                        policy_id: policy_id.clone(),
                    };
                }
            }
        }
        let registry = OperationAssuranceRegistry {
            config: self.config,
            operations: self.operations,
        };
        if registry.config.strict_mutation_classification {
            registry.ensure_complete()?;
        }
        Ok(registry)
    }

    fn operation_mut(
        &mut self,
        kind: GraphqlOperationKind,
        field_name: &str,
    ) -> Result<&mut RegisteredOperation, AssuranceRegistryError> {
        let coordinate = OperationAssuranceCoordinate::new(kind, field_name);
        self.operations
            .get_mut(&coordinate)
            .ok_or(AssuranceRegistryError::UnknownOperation(coordinate))
    }
}

/// Immutable schema operation-assurance registry.
#[derive(Clone, Debug)]
pub struct OperationAssuranceRegistry {
    config: AssuranceSchemaConfig,
    operations: BTreeMap<OperationAssuranceCoordinate, RegisteredOperation>,
}

impl OperationAssuranceRegistry {
    /// Creates a builder from generated operation metadata.
    pub fn builder(
        catalog: &GraphqlOperationCatalog,
        config: AssuranceSchemaConfig,
    ) -> OperationAssuranceRegistryBuilder {
        OperationAssuranceRegistryBuilder::from_catalog(catalog, config)
    }

    /// Returns one resolved field classification.
    pub fn classification(
        &self,
        kind: GraphqlOperationKind,
        field_name: &str,
    ) -> Option<&OperationAssuranceClassification> {
        self.operations
            .get(&OperationAssuranceCoordinate::new(kind, field_name))
            .map(|operation| &operation.classification)
    }

    /// Returns completeness findings for exposed mutations.
    pub fn audit(&self) -> OperationAssuranceAudit {
        OperationAssuranceAudit {
            unclassified_mutations: self
                .operations
                .values()
                .filter(|operation| {
                    operation.coordinate.kind == GraphqlOperationKind::Mutation
                        && operation.classification
                            == OperationAssuranceClassification::Unclassified
                })
                .map(|operation| operation.coordinate.clone())
                .collect(),
        }
    }

    /// Fails when an exposed mutation has neither a requirement nor exemption.
    pub fn ensure_complete(&self) -> Result<(), AssuranceRegistryError> {
        let audit = self.audit();
        if audit.is_complete() {
            Ok(())
        } else {
            Err(AssuranceRegistryError::IncompleteMutations(
                audit.unclassified_mutations,
            ))
        }
    }

    /// Produces deterministic schema directive metadata for all registered fields.
    pub fn schema_metadata(&self) -> Vec<OperationAssuranceSchemaMetadata> {
        self.operations
            .values()
            .map(|operation| OperationAssuranceSchemaMetadata {
                operation_id: operation.operation_id.clone(),
                operation_kind: operation.coordinate.kind,
                root_type: operation.coordinate.kind.root_type().to_string(),
                field_name: operation.coordinate.field_name.clone(),
                field_coordinate: operation.coordinate.field_coordinate(),
                directive: directive_for(operation),
            })
            .collect()
    }

    /// Produces the advisory deterministic client-codegen manifest.
    ///
    /// The manifest never authorizes execution. Server-side enforcement uses
    /// this registry and the current evaluator independently for every field.
    pub fn manifest(&self) -> OperationAssuranceManifest {
        let operations = self
            .operations
            .values()
            .map(OperationAssuranceManifestEntry::from)
            .collect::<Vec<_>>();
        let canonical = serde_json::to_vec(&operations)
            .expect("operation assurance manifest entries always serialize");
        let mut hasher = Sha256::new();
        hasher.update(b"graphql-orm:operation-assurance-manifest:v1\0");
        hasher.update(canonical);
        let fingerprint = format!("{:x}", hasher.finalize());
        OperationAssuranceManifest {
            format_version: OPERATION_ASSURANCE_MANIFEST_VERSION,
            fingerprint,
            operations,
        }
    }

    fn operation(
        &self,
        kind: GraphqlOperationKind,
        field_name: &str,
    ) -> Option<&RegisteredOperation> {
        self.operations
            .get(&OperationAssuranceCoordinate::new(kind, field_name))
    }
}

/// Completeness findings suitable for CI or a schema test.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationAssuranceAudit {
    /// Exposed mutations with neither requirement nor explicit exemption.
    pub unclassified_mutations: Vec<OperationAssuranceCoordinate>,
}

impl OperationAssuranceAudit {
    /// Returns whether every exposed mutation is classified.
    pub fn is_complete(&self) -> bool {
        self.unclassified_mutations.is_empty()
    }

    /// Test helper that panics with deterministic missing coordinates.
    pub fn assert_complete(&self) {
        assert!(
            self.is_complete(),
            "unclassified GraphQL mutations: {}",
            self.unclassified_mutations
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// Schema field metadata carrying a provider-neutral directive use.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationAssuranceSchemaMetadata {
    /// Stable generated fingerprint or host-authored custom operation identity.
    pub operation_id: String,
    /// GraphQL root kind.
    pub operation_kind: GraphqlOperationKind,
    /// Conventional GraphQL root type name.
    pub root_type: String,
    /// Exact schema field name.
    pub field_name: String,
    /// Stable `Root.field` coordinate.
    pub field_coordinate: String,
    /// Directive usage, or `None` while compatibility mode leaves it unclassified.
    pub directive: Option<String>,
}

/// Deterministic, advisory client-codegen manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationAssuranceManifest {
    /// Manifest format version.
    pub format_version: u32,
    /// SHA-256 fingerprint of the canonical ordered entries.
    pub fingerprint: String,
    /// Operations sorted by root kind and exact field identity.
    pub operations: Vec<OperationAssuranceManifestEntry>,
}

impl OperationAssuranceManifest {
    /// Serializes deterministic compact JSON.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    /// Serializes deterministic human-readable JSON.
    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

/// One deterministic manifest entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationAssuranceManifestEntry {
    /// Stable generated fingerprint or host-authored custom operation identity.
    pub operation_id: String,
    /// GraphQL root kind.
    pub operation_kind: GraphqlOperationKind,
    /// Conventional GraphQL root type name.
    pub root_type: String,
    /// Exact schema field name.
    pub field_name: String,
    /// Stable `Root.field` coordinate.
    pub field_coordinate: String,
    /// Generated or custom resolver origin.
    pub origin: OperationAssuranceOrigin,
    /// Actor classification controlling interactive defaults.
    pub actor_class: AssuranceActorClass,
    /// `required`, `exempt`, or `unclassified` plus its associated metadata.
    #[serde(flatten)]
    pub classification: OperationAssuranceClassification,
}

impl From<&RegisteredOperation> for OperationAssuranceManifestEntry {
    fn from(operation: &RegisteredOperation) -> Self {
        Self {
            operation_id: operation.operation_id.clone(),
            operation_kind: operation.coordinate.kind,
            root_type: operation.coordinate.kind.root_type().to_string(),
            field_name: operation.coordinate.field_name.clone(),
            field_coordinate: operation.coordinate.field_coordinate(),
            origin: operation.origin,
            actor_class: operation.actor_class,
            classification: operation.classification.clone(),
        }
    }
}

/// Generic server-side evaluator installed by an integration crate or host.
pub trait AssuranceRequirementEvaluator: Send + Sync + fmt::Debug {
    /// Enforces the requirement against current request context before execution.
    fn enforce(
        &self,
        ctx: &async_graphql::Context<'_>,
        actor_class: AssuranceActorClass,
        policy_id: &str,
    ) -> async_graphql::Result<()>;
}

/// Schema data combining authoritative classifications and an evaluator.
#[derive(Clone)]
pub struct AssuranceEnforcement {
    registry: Arc<OperationAssuranceRegistry>,
    evaluator: Arc<dyn AssuranceRequirementEvaluator>,
}

impl AssuranceEnforcement {
    /// Creates enforcement data suitable for `SchemaBuilder::data`.
    pub fn new(
        registry: Arc<OperationAssuranceRegistry>,
        evaluator: Arc<dyn AssuranceRequirementEvaluator>,
    ) -> Self {
        Self {
            registry,
            evaluator,
        }
    }

    /// Returns the authoritative registry.
    pub fn registry(&self) -> &Arc<OperationAssuranceRegistry> {
        &self.registry
    }
}

impl fmt::Debug for AssuranceEnforcement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssuranceEnforcement")
            .field("registry", &self.registry)
            .field("evaluator", &"[configured]")
            .finish()
    }
}

/// Enforces the declaration for the current generated or custom root field.
///
/// If no `AssuranceEnforcement` is installed, this is a compatibility no-op.
/// Once installed, required fields are evaluated and exemptions pass through.
/// Strict mode rejects an unregistered or unclassified mutation.
pub fn enforce_resolver_assurance(
    ctx: &async_graphql::Context<'_>,
    kind: GraphqlOperationKind,
) -> async_graphql::Result<()> {
    let Some(enforcement) = ctx.data_opt::<AssuranceEnforcement>() else {
        return Ok(());
    };
    let field_name = ctx.field().name();
    let Some(operation) = enforcement.registry.operation(kind, field_name) else {
        return if kind == GraphqlOperationKind::Mutation
            && enforcement.registry.config.strict_mutation_classification
        {
            Err(configuration_error(format!(
                "missing assurance classification for {}.{field_name}",
                kind.root_type()
            )))
        } else {
            Ok(())
        };
    };
    match &operation.classification {
        OperationAssuranceClassification::Required { policy_id } => {
            enforcement
                .evaluator
                .enforce(ctx, operation.actor_class, policy_id)
        }
        OperationAssuranceClassification::Exempt { .. } => Ok(()),
        OperationAssuranceClassification::Unclassified => {
            if kind == GraphqlOperationKind::Mutation
                && enforcement.registry.config.strict_mutation_classification
            {
                Err(configuration_error(format!(
                    "unclassified assurance requirement for {}",
                    operation.coordinate
                )))
            } else {
                Ok(())
            }
        }
    }
}

/// Async-GraphQL guard for custom resolver fields.
#[derive(Clone, Copy, Debug)]
pub struct DeclaredAssuranceGuard {
    kind: GraphqlOperationKind,
}

impl DeclaredAssuranceGuard {
    /// Creates a guard for a query, mutation, or subscription root field.
    pub const fn new(kind: GraphqlOperationKind) -> Self {
        Self { kind }
    }
}

impl Guard for DeclaredAssuranceGuard {
    async fn check(&self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<()> {
        enforce_resolver_assurance(ctx, self.kind)
    }
}

/// Registry construction and strict-completeness errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssuranceRegistryError {
    /// A generated or custom coordinate was registered twice.
    DuplicateOperation(OperationAssuranceCoordinate),
    /// A declaration referenced no registered field.
    UnknownOperation(OperationAssuranceCoordinate),
    /// A stable metadata value was empty, oversized, or unsafe for the manifest.
    InvalidMetadata(&'static str),
    /// Strict mode found one or more unclassified mutations.
    IncompleteMutations(Vec<OperationAssuranceCoordinate>),
}

impl fmt::Display for AssuranceRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateOperation(coordinate) => write!(
                f,
                "duplicate GraphQL operation assurance coordinate: {coordinate}"
            ),
            Self::UnknownOperation(coordinate) => write!(
                f,
                "unknown GraphQL operation assurance coordinate: {coordinate}"
            ),
            Self::InvalidMetadata(field) => {
                write!(f, "invalid operation assurance metadata: {field}")
            }
            Self::IncompleteMutations(_) => {
                f.write_str("one or more GraphQL mutations lack assurance classification")
            }
        }
    }
}

impl std::error::Error for AssuranceRegistryError {}

fn directive_for(operation: &RegisteredOperation) -> Option<String> {
    match &operation.classification {
        OperationAssuranceClassification::Unclassified => None,
        OperationAssuranceClassification::Required { policy_id } => Some(format!(
            "@requiresAssurance(policy: {}, actor: {})",
            graphql_string(policy_id),
            operation.actor_class.directive_value()
        )),
        OperationAssuranceClassification::Exempt { reason } => Some(format!(
            "@assuranceExempt(reason: {}, actor: {})",
            graphql_string(reason),
            operation.actor_class.directive_value()
        )),
    }
}

fn graphql_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization cannot fail")
}

fn validate_policy_id(value: &str) -> Result<(), AssuranceRegistryError> {
    if value.is_empty()
        || value.len() > MAX_POLICY_ID_LENGTH
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err(AssuranceRegistryError::InvalidMetadata("policy_id"));
    }
    Ok(())
}

fn validate_field_name(value: &str) -> Result<(), AssuranceRegistryError> {
    let mut bytes = value.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
        || !bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    {
        return Err(AssuranceRegistryError::InvalidMetadata("field_name"));
    }
    Ok(())
}

fn validate_metadata_value(field: &'static str, value: &str) -> Result<(), AssuranceRegistryError> {
    if value.trim().is_empty()
        || value.len() > MAX_METADATA_VALUE_LENGTH
        || value.chars().any(char::is_control)
    {
        return Err(AssuranceRegistryError::InvalidMetadata(field));
    }
    Ok(())
}

fn configuration_error(internal: String) -> async_graphql::Error {
    crate::graphql::errors::OrmPublicError::new(
        crate::graphql::errors::OrmErrorCode::AuthorizationMisconfigured,
    )
    .with_internal(internal)
    .into_graphql_error()
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use super::*;
    use crate::graphql::orm::{
        GeneratedGraphqlOperationCategory, GeneratedGraphqlOperationDescriptor,
        GraphqlOperationArgumentDescriptor,
    };

    fn catalog() -> GraphqlOperationCatalog {
        static OPERATIONS: OnceLock<Box<[GeneratedGraphqlOperationDescriptor]>> = OnceLock::new();
        let operations = OPERATIONS.get_or_init(|| {
            vec![
                GeneratedGraphqlOperationDescriptor::generated(
                    "example::Record",
                    "Record",
                    "records",
                    "sqlite",
                    GraphqlOperationKind::Query,
                    GeneratedGraphqlOperationCategory::List,
                    "records",
                    vec![],
                    "RecordConnection",
                    "RecordConnection!",
                    "record-shape-v1",
                ),
                GeneratedGraphqlOperationDescriptor::generated(
                    "example::Record",
                    "Record",
                    "records",
                    "sqlite",
                    GraphqlOperationKind::Subscription,
                    GeneratedGraphqlOperationCategory::Subscription,
                    "recordChanged",
                    vec![],
                    "RecordChangedEvent",
                    "RecordChangedEvent!",
                    "record-shape-v1",
                ),
                GeneratedGraphqlOperationDescriptor::generated(
                    "example::Record",
                    "Record",
                    "records",
                    "sqlite",
                    GraphqlOperationKind::Mutation,
                    GeneratedGraphqlOperationCategory::Create,
                    "createRecord",
                    vec![GraphqlOperationArgumentDescriptor::generated(
                        "input",
                        "CreateRecordInput",
                        "CreateRecordInput!",
                    )],
                    "RecordResult",
                    "RecordResult!",
                    "record-shape-v1",
                ),
            ]
            .into_boxed_slice()
        });
        GraphqlOperationCatalog::compose([(operations.as_ref(), true, true)])
    }

    #[test]
    fn legacy_mode_audits_mutations_without_changing_query_or_subscription_defaults() {
        let registry =
            OperationAssuranceRegistry::builder(&catalog(), AssuranceSchemaConfig::legacy())
                .build()
                .unwrap();
        assert_eq!(registry.audit().unclassified_mutations.len(), 1);
        assert_eq!(
            registry.classification(GraphqlOperationKind::Query, "records"),
            Some(&OperationAssuranceClassification::Unclassified)
        );
        assert_eq!(
            registry.classification(GraphqlOperationKind::Mutation, "createRecord"),
            Some(&OperationAssuranceClassification::Unclassified)
        );
        assert_eq!(
            registry.classification(GraphqlOperationKind::Subscription, "recordChanged"),
            Some(&OperationAssuranceClassification::Unclassified)
        );
    }

    #[test]
    fn strict_default_covers_interactive_and_requires_explicit_machine_and_teardown_exemptions() {
        let config = AssuranceSchemaConfig::legacy()
            .with_default_interactive_mutation_policy("interactive.recent-auth")
            .unwrap()
            .with_strict_mutation_classification(true);
        let mut builder = OperationAssuranceRegistry::builder(&catalog(), config.clone());
        builder
            .register_custom(
                "custom:sync:v1",
                GraphqlOperationKind::Mutation,
                "syncRecords",
                AssuranceActorClass::Service,
            )
            .unwrap();
        assert!(matches!(
            builder.clone().build(),
            Err(AssuranceRegistryError::IncompleteMutations(_))
        ));

        builder
            .exempt(
                GraphqlOperationKind::Mutation,
                "syncRecords",
                "service credential has no interactive session",
            )
            .unwrap()
            .register_custom(
                "custom:logout:v1",
                GraphqlOperationKind::Mutation,
                "logoutEverywhere",
                AssuranceActorClass::SafetyTeardown,
            )
            .unwrap()
            .exempt(
                GraphqlOperationKind::Mutation,
                "logoutEverywhere",
                "must remain available to revoke a session",
            )
            .unwrap();
        let registry = builder.build().unwrap();
        registry.audit().assert_complete();
        assert_eq!(
            registry.classification(GraphqlOperationKind::Mutation, "createRecord"),
            Some(&OperationAssuranceClassification::Required {
                policy_id: "interactive.recent-auth".to_string(),
            })
        );
        assert_eq!(
            registry.classification(GraphqlOperationKind::Query, "records"),
            Some(&OperationAssuranceClassification::Unclassified)
        );
        assert_eq!(
            registry.classification(GraphqlOperationKind::Subscription, "recordChanged"),
            Some(&OperationAssuranceClassification::Unclassified)
        );
    }

    #[test]
    fn manifests_and_directives_are_deterministic_and_include_custom_identity() {
        let build = |reverse: bool| {
            let config = AssuranceSchemaConfig::legacy()
                .with_default_interactive_mutation_policy("interactive.recent-auth")
                .unwrap();
            let mut builder = OperationAssuranceRegistry::builder(&catalog(), config);
            let fields = if reverse {
                [("custom:z:v1", "zAction"), ("custom:a:v1", "aAction")]
            } else {
                [("custom:a:v1", "aAction"), ("custom:z:v1", "zAction")]
            };
            for (operation_id, field) in fields {
                builder
                    .register_custom(
                        operation_id,
                        GraphqlOperationKind::Mutation,
                        field,
                        AssuranceActorClass::Interactive,
                    )
                    .unwrap();
            }
            builder
                .exempt(
                    GraphqlOperationKind::Mutation,
                    "zAction",
                    "quoted \"safety\" reason",
                )
                .unwrap();
            builder.build().unwrap()
        };
        let left = build(false);
        let right = build(true);
        assert_eq!(left.manifest(), right.manifest());
        assert_eq!(
            left.manifest().to_json().unwrap(),
            right.manifest().to_json().unwrap()
        );

        let manifest = left.manifest();
        let custom = manifest
            .operations
            .iter()
            .find(|entry| entry.field_name == "aAction")
            .unwrap();
        assert_eq!(custom.operation_id, "custom:a:v1");
        assert_eq!(custom.field_coordinate, "Mutation.aAction");
        assert_eq!(custom.actor_class, AssuranceActorClass::Interactive);
        assert!(matches!(
            custom.classification,
            OperationAssuranceClassification::Required { .. }
        ));

        let metadata = left.schema_metadata();
        assert!(metadata.iter().any(|entry| {
            entry.field_name == "createRecord"
                && entry.directive.as_deref()
                    == Some(
                        "@requiresAssurance(policy: \"interactive.recent-auth\", actor: INTERACTIVE)",
                    )
        }));
        assert!(metadata.iter().any(|entry| {
            entry.field_name == "zAction"
                && entry.directive.as_deref()
                    == Some(
                        "@assuranceExempt(reason: \"quoted \\\"safety\\\" reason\", actor: INTERACTIVE)",
                    )
        }));
        assert!(ASSURANCE_DIRECTIVE_DEFINITIONS.contains("@requiresAssurance"));
        assert!(ASSURANCE_DIRECTIVE_DEFINITIONS.contains("@assuranceExempt"));
    }
}
