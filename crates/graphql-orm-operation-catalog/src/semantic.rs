//! Canonical public GraphQL semantic metadata.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{GeneratedGraphqlOperationCategory, GraphqlOperationCatalog, GraphqlOperationKind};

/// Current semantic-catalogue wire version.
pub const GRAPHQL_SEMANTIC_CATALOG_VERSION: u16 = 2;

/// Router descriptor extension carrying a canonical semantic catalogue.
pub const GRAPHQL_SEMANTIC_CATALOG_EXTENSION_NAME: &str = "graphql-orm.semantic-catalog";

/// Stable semantic-catalogue fingerprint algorithm.
pub const GRAPHQL_SEMANTIC_FINGERPRINT_ALGORITHM: &str =
    "graphql-orm-semantic-canonical-json-sha256-v2";

const MAXIMUM_DESCRIPTION_BYTES: usize = 1_024;
const MAXIMUM_ENTITIES: usize = 4_096;
const MAXIMUM_FIELDS_PER_ENTITY: usize = 2_048;
const MAXIMUM_OPERATIONS: usize = 8_192;
const MAXIMUM_ARGUMENTS: usize = 256;

/// Stable validation error for canonical semantic metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphqlSemanticError {
    message: &'static str,
}

impl GraphqlSemanticError {
    fn new(message: &'static str) -> Self {
        Self { message }
    }

    /// Returns a bounded, content-free diagnostic category.
    pub const fn message(&self) -> &'static str {
        self.message
    }
}

impl std::fmt::Display for GraphqlSemanticError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for GraphqlSemanticError {}

/// Portable aggregate operator advertised by public field metadata.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphqlAggregateOperator {
    /// Count matching rows or non-null values according to the aggregate input.
    Count,
    /// Minimum non-null value.
    Min,
    /// Maximum non-null value.
    Max,
    /// Sum non-null numeric values.
    Sum,
}

/// Portable result family for one aggregate expression.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphqlAggregateValueKind {
    /// Non-negative row or value count.
    Count,
    /// Widened integral value with checked conversion.
    Integral,
    /// Exact fixed-precision decimal value.
    Decimal,
    /// Floating-point value.
    Floating,
    /// Comparable text value, available to minimum and maximum only.
    Text,
}

/// Closed filter operator advertised for one public field.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphqlSemanticFilterOperator {
    /// Equality.
    Equal,
    /// Inequality.
    NotEqual,
    /// Membership in a bounded list.
    In,
    /// Less-than comparison.
    LessThan,
    /// Less-than-or-equal comparison.
    LessThanOrEqual,
    /// Greater-than comparison.
    GreaterThan,
    /// Greater-than-or-equal comparison.
    GreaterThanOrEqual,
    /// Bounded substring containment.
    Contains,
    /// Prefix match.
    StartsWith,
    /// Suffix match.
    EndsWith,
    /// Inclusive two-value range.
    Between,
    /// Null-state predicate.
    IsNull,
    /// Timestamp is before the start of today.
    InPast,
    /// Timestamp is at or after the start of tomorrow.
    InFuture,
    /// Timestamp is within today's half-open calendar range.
    IsToday,
    /// Positive bounded calendar span ending with today.
    RecentDays,
    /// Positive bounded calendar span beginning with today.
    WithinDays,
    /// Inclusive lower bound at a signed day offset from today.
    GteRelative,
    /// Inclusive calendar-date upper bound at a signed day offset from today.
    LteRelative,
}

/// Model-neutral classification inherited by a public semantic field.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphqlSemanticClassification {
    /// Public information.
    Public,
    /// Internal application information.
    #[default]
    Internal,
    /// Confidential user or tenant information.
    Confidential,
    /// Highly restricted information.
    Restricted,
    /// Credentials, keys, or equivalent secret material.
    Secret,
}

/// Whether a semantic field may participate in an external-provider projection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlSemanticExport {
    /// Export is structurally possible, subject to current disclosure policy.
    Exportable,
    /// The field must never cross the resolver/provider boundary.
    NeverExport,
}

/// Closed disclosure metadata for one root result.
///
/// This is descriptive, fingerprinted egress metadata only. It does not
/// authorize the resolver or override entity/field disclosure rules. For an
/// object result, the effective classification is at least as restrictive as
/// every selected field and `NeverExport` remains absolute.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphqlSemanticResultDisclosure {
    /// Minimum classification imposed by the root result.
    pub classification: GraphqlSemanticClassification,
    /// Structural export disposition for the complete root result.
    pub export: GraphqlSemanticExport,
    /// Positive server-owned bound for a scalar/enum list result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_items: Option<u32>,
}

impl GraphqlSemanticResultDisclosure {
    /// Returns the fail-safe default for an unclassified custom scalar result.
    pub const fn fail_safe() -> Self {
        Self {
            classification: GraphqlSemanticClassification::Secret,
            export: GraphqlSemanticExport::NeverExport,
            maximum_items: None,
        }
    }

    /// Builds explicit result disclosure metadata.
    pub const fn new(
        classification: GraphqlSemanticClassification,
        export: GraphqlSemanticExport,
    ) -> Self {
        Self {
            classification,
            export,
            maximum_items: None,
        }
    }

    /// Applies a positive scalar/enum-list ceiling.
    pub const fn with_maximum_items(mut self, maximum_items: u32) -> Self {
        self.maximum_items = Some(maximum_items);
        self
    }
}

/// Public GraphQL type kind without Rust or database identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphqlSemanticTypeKind {
    /// Scalar value.
    Scalar,
    /// Enum value.
    Enum,
    /// Object value.
    Object,
}

/// Closed public GraphQL type reference.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "shape", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphqlSemanticTypeRef {
    /// Named scalar, enum, or object.
    Named {
        /// Public GraphQL type name.
        name: String,
        /// Public type kind.
        kind: GraphqlSemanticTypeKind,
        /// Whether this node accepts or returns null.
        nullable: bool,
    },
    /// List with an item type and an optional server-owned ceiling.
    List {
        /// Whether the list itself may be null.
        nullable: bool,
        /// Required positive ceiling for selectable relationship collections.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        maximum_items: Option<u32>,
        /// Public item type.
        item: Box<GraphqlSemanticTypeRef>,
    },
}

impl GraphqlSemanticTypeRef {
    /// Builds a named public type reference.
    pub fn named(name: impl Into<String>, kind: GraphqlSemanticTypeKind, nullable: bool) -> Self {
        Self::Named {
            name: name.into(),
            kind,
            nullable,
        }
    }

    /// Builds a list reference.
    pub fn list(nullable: bool, maximum_items: Option<u32>, item: Self) -> Self {
        Self::List {
            nullable,
            maximum_items,
            item: Box::new(item),
        }
    }

    fn leaf_kind(&self) -> Option<GraphqlSemanticTypeKind> {
        match self {
            Self::Named { kind, .. } => Some(*kind),
            Self::List { item, .. } => item.leaf_kind(),
        }
    }

    fn is_list(&self) -> bool {
        matches!(self, Self::List { .. })
    }

    fn set_leaf_kind(&mut self, replacement: GraphqlSemanticTypeKind) {
        match self {
            Self::Named { kind, .. } => *kind = replacement,
            Self::List { item, .. } => item.set_leaf_kind(replacement),
        }
    }

    fn validate(&self, depth: usize) -> Result<(), GraphqlSemanticError> {
        if depth > 16 {
            return Err(GraphqlSemanticError::new(
                "semantic type nesting is invalid",
            ));
        }
        match self {
            Self::Named { name, .. } if !valid_graphql_name(name) => {
                Err(GraphqlSemanticError::new("semantic type name is invalid"))
            }
            Self::Named { .. } => Ok(()),
            Self::List {
                maximum_items,
                item,
                ..
            } => {
                if maximum_items.is_some_and(|limit| limit == 0) {
                    return Err(GraphqlSemanticError::new("semantic list bound is invalid"));
                }
                item.validate(depth + 1)
            }
        }
    }
}

/// Public relationship cardinality.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlSemanticRelationshipCardinality {
    /// At most one related object.
    One,
    /// A bounded collection of related objects.
    Many,
}

/// Closed contract for how a Many relationship enforces its item ceiling.
///
/// This is descriptive, fingerprinted catalogue metadata. It does not
/// authorize a resolver, override row/field policy, or invent a GraphQL
/// argument. Execution must honor the stored mode instead of inferring it
/// from a field name.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphqlSemanticCollectionBound {
    /// The maximum is enforced by one unique typed GraphQL paging argument.
    Pageable {
        /// Public argument that carries the trusted page size.
        argument_name: String,
    },
    /// The maximum is guaranteed by the resolver and has no caller-controlled
    /// paging argument.
    ServerFixed {
        /// Authoritative positive item ceiling.
        maximum_items: u32,
    },
}

impl GraphqlSemanticCollectionBound {
    /// Creates a pageable bound for one named paging argument.
    pub fn pageable(argument_name: impl Into<String>) -> Self {
        Self::Pageable {
            argument_name: argument_name.into(),
        }
    }

    /// Creates a resolver-owned fixed ceiling.
    pub const fn server_fixed(maximum_items: u32) -> Self {
        Self::ServerFixed { maximum_items }
    }

    /// Returns the pageable argument name when this bound is pageable.
    pub fn page_argument_name(&self) -> Option<&str> {
        match self {
            Self::Pageable { argument_name } => Some(argument_name.as_str()),
            Self::ServerFixed { .. } => None,
        }
    }

    /// Returns the authoritative fixed ceiling when the resolver owns it.
    pub const fn server_fixed_maximum(&self) -> Option<u32> {
        match self {
            Self::ServerFixed { maximum_items } => Some(*maximum_items),
            Self::Pageable { .. } => None,
        }
    }

    /// Whether the model may choose a smaller page size through the plan.
    pub const fn model_may_select_maximum(&self) -> bool {
        matches!(self, Self::Pageable { .. })
    }
}

/// One public argument in a semantic operation or relationship field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphqlSemanticArgumentDescriptor {
    /// Public GraphQL argument name.
    pub graphql_name: String,
    /// Model-safe semantic description.
    pub description: String,
    /// Public GraphQL type shape.
    pub type_ref: GraphqlSemanticTypeRef,
}

/// Semantic relationship contract for one selectable field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphqlSemanticRelationshipDescriptor {
    /// Public target GraphQL object name.
    pub target: String,
    /// Relationship cardinality.
    pub cardinality: GraphqlSemanticRelationshipCardinality,
    /// Typed public relationship arguments.
    pub arguments: Vec<GraphqlSemanticArgumentDescriptor>,
    /// Required collection-bound contract when [`Self::cardinality`] is Many.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_bound: Option<GraphqlSemanticCollectionBound>,
}

/// Canonical model- and documentation-facing public field descriptor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphqlSemanticFieldMetadata {
    /// Exact public GraphQL field name.
    pub field_name: String,
    /// Model-safe semantic description.
    pub description: String,
    /// Public GraphQL type shape.
    pub type_ref: GraphqlSemanticTypeRef,
    /// Whether a closed query plan may select the field.
    pub selectable: bool,
    /// Supported filter operators.
    pub filter_operators: Vec<GraphqlSemanticFilterOperator>,
    /// Whether generated ordering accepts the field.
    pub sortable: bool,
    /// Whether generated grouping accepts the field.
    pub groupable: bool,
    /// Supported aggregate operators.
    pub aggregate_operators: Vec<GraphqlAggregateOperator>,
    /// Aggregate result family when aggregation is supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate_value_kind: Option<GraphqlAggregateValueKind>,
    /// Relationship metadata when this field traverses another object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship: Option<GraphqlSemanticRelationshipDescriptor>,
    /// Inherited minimum classification.
    pub classification: GraphqlSemanticClassification,
    /// Structural export disposition.
    pub export: GraphqlSemanticExport,
    /// Whether an authoritative field policy is present.
    pub has_field_policy: bool,
}

/// Canonical semantic metadata for one public entity/type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphqlEntitySemanticMetadata {
    /// Public GraphQL entity/type identity.
    pub entity_name: String,
    /// Model-safe entity description.
    pub description: String,
    /// Default classification inherited by fields.
    pub default_classification: GraphqlSemanticClassification,
    /// Public fields only, in declaration order.
    pub fields: Box<[GraphqlSemanticFieldMetadata]>,
}

/// Durability advertised by one semantically described subscription.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlSubscriptionReplayMode {
    /// Live delivery may be missed while a client or worker is disconnected.
    BestEffort,
    /// An opaque cursor and captured watermark support bounded replay followed
    /// by live delivery, including an authoritative reset-required outcome.
    ReplayThenLive,
}

/// Closed condition operator available to a bounded subscription observer.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlSubscriptionConditionOperator {
    /// Exact equality with a typed fixed value.
    Equal,
    /// Inequality with a typed fixed value.
    NotEqual,
    /// Numeric or ordered-value less-than comparison.
    LessThan,
    /// Numeric or ordered-value less-than-or-equal comparison.
    LessThanOrEqual,
    /// Numeric or ordered-value greater-than comparison.
    GreaterThan,
    /// Numeric or ordered-value greater-than-or-equal comparison.
    GreaterThanOrEqual,
}

/// One selectable event field admitted into a server-validated wait condition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphqlSubscriptionConditionField {
    /// Public event-field name.
    pub field_name: String,
    /// Closed supported operators, ordered canonically.
    pub operators: Vec<GraphqlSubscriptionConditionOperator>,
}

/// Bounded observation semantics declared beside a subscription root.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphqlSubscriptionObservationDescriptor {
    /// Truthful delivery/replay capability.
    pub replay_mode: GraphqlSubscriptionReplayMode,
    /// Positive host-owned maximum duration of one observation, when bounded
    /// wait registration is supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_duration_seconds: Option<u32>,
    /// Positive host-owned maximum number of events examined, when bounded
    /// wait registration is supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_events: Option<u32>,
    /// Event fields and operators permitted in a closed completion condition.
    pub condition_fields: Vec<GraphqlSubscriptionConditionField>,
}

/// Whether an operation came from generated or custom resolver metadata.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlSemanticOperationSource {
    /// Generated by `GraphQLOperations`.
    Generated,
    /// Declared beside a handwritten resolver.
    Custom,
}

/// Closed AI execution classification for one public mutation root.
///
/// This is descriptive capability metadata, not authority. Resolver, tenant,
/// row, field, assurance, approval, and database policy remain authoritative
/// at execution time. Mutations default to [`Self::Prohibited`].
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiMutationExecutionPolicy {
    /// A bounded low-consequence mutation may execute under freshly resolved
    /// ordinary application authority without a per-call human approval.
    Automatic,
    /// The exact target, arguments, preview, and authority require one
    /// expiring human approval before a single execution.
    ApprovalRequired,
    /// The mutation is absent from executable AI capabilities.
    #[default]
    Prohibited,
}

/// Canonical semantic descriptor for one public root operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphqlSemanticOperationDescriptor {
    /// Root operation kind.
    pub kind: GraphqlOperationKind,
    /// Exact public GraphQL root field name.
    pub field_name: String,
    /// Generated or custom source.
    pub source: GraphqlSemanticOperationSource,
    /// Stable generated category when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_category: Option<GeneratedGraphqlOperationCategory>,
    /// Public entity identity owning a generated operation.
    ///
    /// This is a GraphQL semantic name, never a table, column, or Rust path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_entity_name: Option<String>,
    /// Model-safe operation description.
    pub description: String,
    /// Typed public arguments.
    pub arguments: Vec<GraphqlSemanticArgumentDescriptor>,
    /// Public result type shape.
    pub result_type: GraphqlSemanticTypeRef,
    /// Optional root-level result disclosure contract.
    ///
    /// Custom scalar and enum results always carry this field. Object results
    /// may carry it only to tighten their selected-field disclosure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_disclosure: Option<GraphqlSemanticResultDisclosure>,
    /// Whether this exact operation is composed into the finished root.
    pub is_exposed: bool,
    /// Whether an authoritative fixed or dynamic authorization policy exists.
    pub has_authorization_policy: bool,
    /// Closed AI execution classification. Present only for mutation roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_mutation_execution: Option<AiMutationExecutionPolicy>,
    /// Subscription observation semantics. Absent for queries and mutations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_observation: Option<GraphqlSubscriptionObservationDescriptor>,
    /// Stable semantic fingerprint for this operation.
    pub fingerprint: String,
}

/// Metadata trait implemented beside a handwritten resolver root.
pub trait GraphqlCustomOperationMetadata {
    /// Returns canonical semantic declarations authored beside the resolver.
    fn graphql_custom_operations() -> &'static [GraphqlSemanticOperationDescriptor];

    /// Returns direct handwritten result-object semantics authored beside the
    /// resolver declarations.
    ///
    /// Macro-generated implementations provide this automatically. The empty
    /// default preserves compatibility for manually implemented metadata.
    fn graphql_custom_result_types() -> &'static [GraphqlEntitySemanticMetadata] {
        &[]
    }
}

/// Metadata trait implemented by a described handwritten GraphQL object.
pub trait GraphqlSemanticObjectMetadata {
    /// Returns the canonical public object-field semantic declaration.
    fn graphql_semantic_object() -> &'static GraphqlEntitySemanticMetadata;
}

/// Common metadata boundary for a GraphQL entity or handwritten result type.
///
/// Proc-macro generated custom roots use this trait to collect the direct
/// result object without a second `schema_roots!` semantic-type list.
pub trait GraphqlSemanticResultTypeMetadata {
    /// Returns the canonical public result-object semantic declaration.
    fn graphql_semantic_result_type() -> &'static GraphqlEntitySemanticMetadata;
}

/// Versioned canonical semantic graph for one composed GraphQL schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphqlSemanticCatalog {
    /// Wire version.
    pub version: u16,
    /// Public entities ordered by GraphQL name.
    pub entities: Vec<GraphqlEntitySemanticMetadata>,
    /// Public operations ordered by root coordinate.
    pub operations: Vec<GraphqlSemanticOperationDescriptor>,
    /// Stable fingerprint over the complete versioned semantic graph.
    pub fingerprint: String,
}

impl GraphqlSemanticCatalog {
    /// Composes generated entity and operation semantics.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid, duplicate, unsafe, or excessive metadata.
    pub fn compose(
        entities: impl IntoIterator<Item = GraphqlEntitySemanticMetadata>,
        operation_catalog: &GraphqlOperationCatalog,
    ) -> Result<Self, GraphqlSemanticError> {
        let entities = normalize_composed_entities(entities);
        let operations = operation_catalog
            .operations()
            .iter()
            .filter(|operation| operation.is_exposed())
            .map(GraphqlSemanticOperationDescriptor::from_generated)
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(entities, operations)
    }

    /// Composes generated semantics with explicitly declared custom roots.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate coordinates or any invalid metadata.
    pub fn compose_with_custom(
        entities: impl IntoIterator<Item = GraphqlEntitySemanticMetadata>,
        operation_catalog: &GraphqlOperationCatalog,
        custom: impl IntoIterator<Item = GraphqlSemanticOperationDescriptor>,
    ) -> Result<Self, GraphqlSemanticError> {
        let entities = normalize_composed_entities(entities);
        let mut operations = operation_catalog
            .operations()
            .iter()
            .filter(|operation| operation.is_exposed())
            .map(GraphqlSemanticOperationDescriptor::from_generated)
            .collect::<Result<Vec<_>, _>>()?;
        operations.extend(custom);
        Self::new(entities, operations)
    }

    fn new(
        entities: Vec<GraphqlEntitySemanticMetadata>,
        mut operations: Vec<GraphqlSemanticOperationDescriptor>,
    ) -> Result<Self, GraphqlSemanticError> {
        operations.sort_by(|left, right| {
            (left.kind, left.field_name.as_str()).cmp(&(right.kind, right.field_name.as_str()))
        });
        let mut catalog = Self {
            version: GRAPHQL_SEMANTIC_CATALOG_VERSION,
            entities,
            operations,
            fingerprint: String::new(),
        };
        catalog.fingerprint = catalog.compute_fingerprint();
        catalog.validate()?;
        Ok(catalog)
    }

    /// Validates the complete semantic graph and its fingerprint.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported versions, malformed public metadata,
    /// duplicate identities, bounds violations, or a stale fingerprint.
    pub fn validate(&self) -> Result<(), GraphqlSemanticError> {
        if self.version != GRAPHQL_SEMANTIC_CATALOG_VERSION {
            return Err(GraphqlSemanticError::new(
                "semantic catalogue version is unsupported",
            ));
        }
        if self.entities.len() > MAXIMUM_ENTITIES || self.operations.len() > MAXIMUM_OPERATIONS {
            return Err(GraphqlSemanticError::new("semantic catalogue is too large"));
        }
        if self
            .entities
            .windows(2)
            .any(|pair| pair[0].entity_name >= pair[1].entity_name)
        {
            return Err(GraphqlSemanticError::new(
                "semantic entity ordering is not canonical",
            ));
        }
        if self.operations.windows(2).any(|pair| {
            (pair[0].kind, pair[0].field_name.as_str())
                >= (pair[1].kind, pair[1].field_name.as_str())
        }) {
            return Err(GraphqlSemanticError::new(
                "semantic operation ordering is not canonical",
            ));
        }
        let mut entity_names = BTreeSet::new();
        for entity in &self.entities {
            validate_entity(entity)?;
            if !entity_names.insert(&entity.entity_name) {
                return Err(GraphqlSemanticError::new("semantic entity is duplicated"));
            }
        }
        let mut coordinates = BTreeSet::new();
        for operation in &self.operations {
            validate_operation(operation)?;
            if let Some(entity_name) = &operation.generated_entity_name
                && !entity_names.contains(entity_name)
            {
                return Err(GraphqlSemanticError::new(
                    "generated semantic operation entity is absent",
                ));
            }
            if !coordinates.insert((operation.kind, &operation.field_name)) {
                return Err(GraphqlSemanticError::new(
                    "semantic operation is duplicated",
                ));
            }
        }
        if !valid_fingerprint(&self.fingerprint) || self.compute_fingerprint() != self.fingerprint {
            return Err(GraphqlSemanticError::new(
                "semantic catalogue fingerprint is stale",
            ));
        }
        Ok(())
    }

    /// Encodes a strict canonical extension payload.
    ///
    /// # Errors
    ///
    /// Returns an error when validation fails.
    pub fn extension_payload(&self) -> Result<serde_json::Value, GraphqlSemanticError> {
        self.validate()?;
        serde_json::to_value(self)
            .map_err(|_| GraphqlSemanticError::new("semantic catalogue encoding failed"))
    }

    /// Decodes and validates a semantic extension payload.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, unsupported, or stale payloads.
    pub fn from_extension_payload(
        payload: serde_json::Value,
    ) -> Result<Self, GraphqlSemanticError> {
        let catalog: Self = serde_json::from_value(payload)
            .map_err(|_| GraphqlSemanticError::new("semantic catalogue payload is invalid"))?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// Creates the generic router-protocol extension carrier.
    ///
    /// # Errors
    ///
    /// Returns an error when semantic or protocol validation fails.
    #[cfg(feature = "router-protocol")]
    pub fn router_protocol_extension(
        &self,
    ) -> Result<graphql_orm_router_protocol::DescriptorExtension, GraphqlSemanticError> {
        graphql_orm_router_protocol::DescriptorExtension::new(
            GRAPHQL_SEMANTIC_CATALOG_EXTENSION_NAME,
            GRAPHQL_SEMANTIC_CATALOG_VERSION,
            self.extension_payload()?,
        )
        .map_err(|_| GraphqlSemanticError::new("semantic catalogue extension is invalid"))
    }

    fn compute_fingerprint(&self) -> String {
        #[derive(Serialize)]
        struct FingerprintInput<'a> {
            algorithm: &'static str,
            version: u16,
            entities: &'a [GraphqlEntitySemanticMetadata],
            operations: Vec<OperationFingerprintInput<'a>>,
        }
        #[derive(Serialize)]
        struct OperationFingerprintInput<'a> {
            kind: GraphqlOperationKind,
            field_name: &'a str,
            source: GraphqlSemanticOperationSource,
            generated_category: Option<GeneratedGraphqlOperationCategory>,
            generated_entity_name: &'a Option<String>,
            description: &'a str,
            arguments: &'a [GraphqlSemanticArgumentDescriptor],
            result_type: &'a GraphqlSemanticTypeRef,
            result_disclosure: &'a Option<GraphqlSemanticResultDisclosure>,
            is_exposed: bool,
            has_authorization_policy: bool,
            ai_mutation_execution: Option<AiMutationExecutionPolicy>,
            subscription_observation: &'a Option<GraphqlSubscriptionObservationDescriptor>,
        }
        let input = FingerprintInput {
            algorithm: GRAPHQL_SEMANTIC_FINGERPRINT_ALGORITHM,
            version: self.version,
            entities: &self.entities,
            operations: self
                .operations
                .iter()
                .map(|operation| OperationFingerprintInput {
                    kind: operation.kind,
                    field_name: &operation.field_name,
                    source: operation.source,
                    generated_category: operation.generated_category,
                    generated_entity_name: &operation.generated_entity_name,
                    description: &operation.description,
                    arguments: &operation.arguments,
                    result_type: &operation.result_type,
                    result_disclosure: &operation.result_disclosure,
                    is_exposed: operation.is_exposed,
                    has_authorization_policy: operation.has_authorization_policy,
                    ai_mutation_execution: operation.ai_mutation_execution,
                    subscription_observation: &operation.subscription_observation,
                })
                .collect(),
        };
        let value = serde_json::to_value(input).expect("semantic fingerprint input serializes");
        hex_sha256(&canonical_json_bytes(&value))
    }
}

impl GraphqlSemanticOperationDescriptor {
    fn from_generated(
        operation: &crate::GraphqlResolverOperationDescriptor,
    ) -> Result<Self, GraphqlSemanticError> {
        let generated = operation.generated();
        let arguments = generated
            .arguments()
            .iter()
            .map(|argument| {
                Ok(GraphqlSemanticArgumentDescriptor {
                    graphql_name: argument.graphql_name().to_owned(),
                    description: argument.description().to_owned(),
                    type_ref: parse_graphql_type(argument.graphql_type())?,
                })
            })
            .collect::<Result<Vec<_>, GraphqlSemanticError>>()?;
        let mut descriptor = Self {
            kind: operation.kind(),
            field_name: operation.field_name().to_owned(),
            source: GraphqlSemanticOperationSource::Generated,
            generated_category: Some(operation.category()),
            generated_entity_name: Some(operation.entity_name().to_owned()),
            description: operation.description().to_owned(),
            arguments,
            result_type: parse_graphql_type(operation.graphql_result_type())?,
            result_disclosure: None,
            is_exposed: operation.is_exposed(),
            has_authorization_policy: !matches!(
                operation.authorization(),
                crate::GraphqlAuthorizationRequirement::Public
            ),
            ai_mutation_execution: (operation.kind() == GraphqlOperationKind::Mutation)
                .then_some(operation.ai_mutation_execution()),
            subscription_observation: (operation.kind() == GraphqlOperationKind::Subscription)
                .then_some(GraphqlSubscriptionObservationDescriptor {
                    replay_mode: GraphqlSubscriptionReplayMode::BestEffort,
                    maximum_duration_seconds: None,
                    maximum_events: None,
                    condition_fields: Vec::new(),
                }),
            fingerprint: String::new(),
        };
        descriptor.fingerprint = descriptor.compute_fingerprint();
        Ok(descriptor)
    }

    /// Constructs a validated custom operation declaration.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, descriptions, arguments, or types.
    #[allow(clippy::too_many_arguments)]
    pub fn custom(
        kind: GraphqlOperationKind,
        field_name: impl Into<String>,
        description: impl Into<String>,
        arguments: Vec<GraphqlSemanticArgumentDescriptor>,
        result_type: GraphqlSemanticTypeRef,
        has_authorization_policy: bool,
    ) -> Result<Self, GraphqlSemanticError> {
        let result_disclosure = result_type
            .leaf_kind()
            .is_some_and(|kind| {
                matches!(
                    kind,
                    GraphqlSemanticTypeKind::Scalar | GraphqlSemanticTypeKind::Enum
                )
            })
            .then_some(GraphqlSemanticResultDisclosure::fail_safe());
        let mut descriptor = Self {
            kind,
            field_name: field_name.into(),
            source: GraphqlSemanticOperationSource::Custom,
            generated_category: None,
            generated_entity_name: None,
            description: description.into(),
            arguments,
            result_type,
            result_disclosure,
            is_exposed: true,
            has_authorization_policy,
            ai_mutation_execution: (kind == GraphqlOperationKind::Mutation)
                .then_some(AiMutationExecutionPolicy::Prohibited),
            subscription_observation: None,
            fingerprint: String::new(),
        };
        validate_operation_structure(&descriptor)?;
        descriptor.fingerprint = descriptor.compute_fingerprint();
        Ok(descriptor)
    }

    /// Attaches an explicit root result disclosure contract.
    ///
    /// The contract may tighten object-field disclosure but never weakens it.
    /// A secret result cannot be marked exportable, and a list ceiling is
    /// accepted only for a scalar or enum list result.
    ///
    /// # Errors
    ///
    /// Returns an error when the result contract is malformed or incompatible
    /// with the declared GraphQL result shape.
    pub fn with_result_disclosure(
        mut self,
        disclosure: GraphqlSemanticResultDisclosure,
    ) -> Result<Self, GraphqlSemanticError> {
        self.result_disclosure = Some(disclosure);
        self.fingerprint.clear();
        validate_operation_structure(&self)?;
        self.fingerprint = self.compute_fingerprint();
        Ok(self)
    }

    /// Overrides the semantic leaf kind for a custom scalar or enum wrapper.
    ///
    /// This does not change the public GraphQL type name or SDL signature. It
    /// exists for custom scalar/enum Rust types that cannot be identified from
    /// their GraphQL name alone by the metadata macro.
    ///
    /// # Errors
    ///
    /// Returns an error for generated operations or an object override.
    pub fn with_custom_result_leaf_kind(
        mut self,
        kind: GraphqlSemanticTypeKind,
    ) -> Result<Self, GraphqlSemanticError> {
        if self.source != GraphqlSemanticOperationSource::Custom
            || kind == GraphqlSemanticTypeKind::Object
        {
            return Err(GraphqlSemanticError::new(
                "custom result kind override is invalid",
            ));
        }
        self.result_type.set_leaf_kind(kind);
        if self.result_disclosure.is_none() {
            self.result_disclosure = Some(GraphqlSemanticResultDisclosure::fail_safe());
        }
        self.fingerprint.clear();
        validate_operation_structure(&self)?;
        self.fingerprint = self.compute_fingerprint();
        Ok(self)
    }

    /// Classifies one custom mutation for AI execution.
    ///
    /// This declaration does not register the operation, authorize a user,
    /// satisfy resolver policy, or grant approval. An omitted classification
    /// remains [`AiMutationExecutionPolicy::Prohibited`].
    ///
    /// # Errors
    ///
    /// Returns an error when attached to a query or subscription.
    pub fn with_ai_mutation_execution(
        mut self,
        policy: AiMutationExecutionPolicy,
    ) -> Result<Self, GraphqlSemanticError> {
        if self.kind != GraphqlOperationKind::Mutation {
            return Err(GraphqlSemanticError::new(
                "AI mutation execution policy is invalid for this operation",
            ));
        }
        self.ai_mutation_execution = Some(policy);
        self.fingerprint.clear();
        validate_operation_structure(&self)?;
        self.fingerprint = self.compute_fingerprint();
        Ok(self)
    }

    /// Attaches bounded observation semantics to a custom subscription.
    ///
    /// This declaration remains descriptive. A durable waiter must separately
    /// bind an authoritative runtime replay source for the exact operation.
    ///
    /// # Errors
    ///
    /// Returns an error when used on a non-subscription operation or when the
    /// observation contract is malformed or unbounded.
    pub fn with_subscription_observation(
        mut self,
        observation: GraphqlSubscriptionObservationDescriptor,
    ) -> Result<Self, GraphqlSemanticError> {
        self.subscription_observation = Some(observation);
        self.fingerprint.clear();
        validate_operation_structure(&self)?;
        self.fingerprint = self.compute_fingerprint();
        Ok(self)
    }

    fn compute_fingerprint(&self) -> String {
        let mut value = serde_json::to_value(self).expect("semantic operation serializes");
        value
            .as_object_mut()
            .expect("semantic operation is object")
            .remove("fingerprint");
        hex_sha256(&canonical_json_bytes(&value))
    }
}

fn validate_entity(entity: &GraphqlEntitySemanticMetadata) -> Result<(), GraphqlSemanticError> {
    if !valid_graphql_name(&entity.entity_name) {
        return Err(GraphqlSemanticError::new("semantic entity name is invalid"));
    }
    validate_description(&entity.description)?;
    if entity.fields.len() > MAXIMUM_FIELDS_PER_ENTITY {
        return Err(GraphqlSemanticError::new(
            "semantic entity has too many fields",
        ));
    }
    let mut fields = BTreeSet::new();
    for field in &entity.fields {
        if !valid_graphql_name(&field.field_name) || !fields.insert(&field.field_name) {
            return Err(GraphqlSemanticError::new(
                "semantic field identity is invalid",
            ));
        }
        validate_description(&field.description)?;
        field.type_ref.validate(0)?;
        if field.classification < entity.default_classification {
            return Err(GraphqlSemanticError::new(
                "semantic field weakens entity classification",
            ));
        }
        if field.classification == GraphqlSemanticClassification::Secret
            && field.export != GraphqlSemanticExport::NeverExport
        {
            return Err(GraphqlSemanticError::new(
                "secret semantic field is exportable",
            ));
        }
        validate_sorted_unique(&field.filter_operators)?;
        validate_sorted_unique(&field.aggregate_operators)?;
        if field
            .aggregate_operators
            .contains(&GraphqlAggregateOperator::Sum)
            && !matches!(
                field.aggregate_value_kind,
                Some(
                    GraphqlAggregateValueKind::Integral
                        | GraphqlAggregateValueKind::Decimal
                        | GraphqlAggregateValueKind::Floating
                )
            )
        {
            return Err(GraphqlSemanticError::new(
                "semantic sum result kind is invalid",
            ));
        }
        if let Some(relationship) = &field.relationship {
            if !valid_graphql_name(&relationship.target)
                || relationship.arguments.len() > MAXIMUM_ARGUMENTS
            {
                return Err(GraphqlSemanticError::new(
                    "semantic relationship is invalid",
                ));
            }
            validate_arguments(&relationship.arguments)?;
            validate_collection_bound(field, relationship)?;
        }
    }
    Ok(())
}

fn normalize_composed_entities(
    entities: impl IntoIterator<Item = GraphqlEntitySemanticMetadata>,
) -> Vec<GraphqlEntitySemanticMetadata> {
    let mut entities = entities.into_iter().collect::<Vec<_>>();
    entities.sort_by(|left, right| left.entity_name.cmp(&right.entity_name));
    entities.dedup_by(|right, left| right == left);
    entities
}

fn validate_operation(
    operation: &GraphqlSemanticOperationDescriptor,
) -> Result<(), GraphqlSemanticError> {
    validate_operation_structure(operation)?;
    if !valid_fingerprint(&operation.fingerprint)
        || operation.compute_fingerprint() != operation.fingerprint
    {
        return Err(GraphqlSemanticError::new(
            "semantic operation fingerprint is stale",
        ));
    }
    Ok(())
}

fn validate_operation_structure(
    operation: &GraphqlSemanticOperationDescriptor,
) -> Result<(), GraphqlSemanticError> {
    if !valid_graphql_name(&operation.field_name) {
        return Err(GraphqlSemanticError::new(
            "semantic operation name is invalid",
        ));
    }
    validate_description(&operation.description)?;
    if !operation.is_exposed {
        return Err(GraphqlSemanticError::new(
            "semantic operation is not publicly exposed",
        ));
    }
    if operation.arguments.len() > MAXIMUM_ARGUMENTS {
        return Err(GraphqlSemanticError::new(
            "semantic operation has too many arguments",
        ));
    }
    validate_arguments(&operation.arguments)?;
    operation.result_type.validate(0)?;
    if let Some(disclosure) = operation.result_disclosure {
        if disclosure.classification == GraphqlSemanticClassification::Secret
            && disclosure.export != GraphqlSemanticExport::NeverExport
        {
            return Err(GraphqlSemanticError::new(
                "secret semantic operation result is exportable",
            ));
        }
        if disclosure.maximum_items.is_some_and(|maximum| maximum == 0) {
            return Err(GraphqlSemanticError::new(
                "semantic operation result bound is invalid",
            ));
        }
        if disclosure.maximum_items.is_some()
            && (!operation.result_type.is_list()
                || !matches!(
                    operation.result_type.leaf_kind(),
                    Some(GraphqlSemanticTypeKind::Scalar | GraphqlSemanticTypeKind::Enum)
                ))
        {
            return Err(GraphqlSemanticError::new(
                "semantic operation result bound is incompatible",
            ));
        }
        if disclosure.export == GraphqlSemanticExport::Exportable
            && operation.result_type.is_list()
            && matches!(
                operation.result_type.leaf_kind(),
                Some(GraphqlSemanticTypeKind::Scalar | GraphqlSemanticTypeKind::Enum)
            )
            && disclosure.maximum_items.is_none()
        {
            return Err(GraphqlSemanticError::new(
                "exportable semantic scalar-list result is unbounded",
            ));
        }
    }
    if operation.source == GraphqlSemanticOperationSource::Custom
        && matches!(
            operation.result_type.leaf_kind(),
            Some(GraphqlSemanticTypeKind::Scalar | GraphqlSemanticTypeKind::Enum)
        )
        && operation.result_disclosure.is_none()
    {
        return Err(GraphqlSemanticError::new(
            "custom scalar result lacks disclosure metadata",
        ));
    }
    match (&operation.subscription_observation, operation.kind) {
        (Some(observation), GraphqlOperationKind::Subscription) => {
            validate_subscription_observation(observation)?;
        }
        (Some(_), _) => {
            return Err(GraphqlSemanticError::new(
                "non-subscription operation has observation metadata",
            ));
        }
        (None, _) => {}
    }
    match (operation.ai_mutation_execution, operation.kind) {
        (Some(_), GraphqlOperationKind::Mutation)
        | (None, GraphqlOperationKind::Query)
        | (None, GraphqlOperationKind::Subscription) => {}
        (Some(_), _) => {
            return Err(GraphqlSemanticError::new(
                "non-mutation operation has AI mutation execution policy",
            ));
        }
        (None, GraphqlOperationKind::Mutation) => {
            return Err(GraphqlSemanticError::new(
                "mutation operation lacks AI execution policy",
            ));
        }
    }
    match operation.source {
        GraphqlSemanticOperationSource::Generated
            if operation.generated_category.is_none()
                || operation
                    .generated_entity_name
                    .as_deref()
                    .is_none_or(|name| !valid_graphql_name(name)) =>
        {
            return Err(GraphqlSemanticError::new(
                "semantic operation source is invalid",
            ));
        }
        GraphqlSemanticOperationSource::Custom
            if operation.generated_category.is_some()
                || operation.generated_entity_name.is_some() =>
        {
            return Err(GraphqlSemanticError::new(
                "semantic operation source is invalid",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn validate_subscription_observation(
    observation: &GraphqlSubscriptionObservationDescriptor,
) -> Result<(), GraphqlSemanticError> {
    let bounded = match (
        observation.maximum_duration_seconds,
        observation.maximum_events,
    ) {
        (Some(duration), Some(events)) if duration > 0 && events > 0 => true,
        (None, None) => false,
        _ => {
            return Err(GraphqlSemanticError::new(
                "subscription observation bounds are invalid",
            ));
        }
    };
    if observation.replay_mode == GraphqlSubscriptionReplayMode::ReplayThenLive && !bounded {
        return Err(GraphqlSemanticError::new(
            "replayable subscription observation is unbounded",
        ));
    }
    if observation.condition_fields.len() > MAXIMUM_FIELDS_PER_ENTITY
        || observation
            .condition_fields
            .windows(2)
            .any(|pair| pair[0].field_name >= pair[1].field_name)
    {
        return Err(GraphqlSemanticError::new(
            "subscription condition fields are not canonical",
        ));
    }
    for field in &observation.condition_fields {
        if !valid_graphql_name(&field.field_name) || field.operators.is_empty() {
            return Err(GraphqlSemanticError::new(
                "subscription condition field is invalid",
            ));
        }
        validate_sorted_unique(&field.operators)?;
    }
    Ok(())
}

fn validate_arguments(
    arguments: &[GraphqlSemanticArgumentDescriptor],
) -> Result<(), GraphqlSemanticError> {
    let mut names = BTreeSet::new();
    for argument in arguments {
        if !valid_graphql_name(&argument.graphql_name) || !names.insert(&argument.graphql_name) {
            return Err(GraphqlSemanticError::new(
                "semantic argument identity is invalid",
            ));
        }
        validate_description(&argument.description)?;
        argument.type_ref.validate(0)?;
    }
    Ok(())
}

fn validate_description(value: &str) -> Result<(), GraphqlSemanticError> {
    let invalid_character = |character: char| {
        character.is_control()
            || matches!(
                character,
                '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'
            )
    };
    if value.trim().is_empty()
        || value.trim() != value
        || value.len() > MAXIMUM_DESCRIPTION_BYTES
        || value.chars().any(invalid_character)
    {
        return Err(GraphqlSemanticError::new("semantic description is invalid"));
    }
    Ok(())
}

fn validate_sorted_unique<T: Ord>(values: &[T]) -> Result<(), GraphqlSemanticError> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(GraphqlSemanticError::new(
            "semantic capability set is not canonical",
        ));
    }
    Ok(())
}

fn valid_graphql_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_collection_bound(
    field: &GraphqlSemanticFieldMetadata,
    relationship: &GraphqlSemanticRelationshipDescriptor,
) -> Result<(), GraphqlSemanticError> {
    match (
        relationship.cardinality,
        relationship.collection_bound.as_ref(),
    ) {
        (GraphqlSemanticRelationshipCardinality::One, None) => Ok(()),
        (GraphqlSemanticRelationshipCardinality::One, Some(_)) => Err(GraphqlSemanticError::new(
            "one-to-one relationship cannot declare a collection bound",
        )),
        (GraphqlSemanticRelationshipCardinality::Many, None) => Err(GraphqlSemanticError::new(
            "semantic relationship collection bound is missing",
        )),
        (
            GraphqlSemanticRelationshipCardinality::Many,
            Some(GraphqlSemanticCollectionBound::Pageable { argument_name }),
        ) => {
            if !valid_graphql_name(argument_name)
                || !relationship
                    .arguments
                    .iter()
                    .any(|argument| argument.graphql_name == *argument_name)
            {
                return Err(GraphqlSemanticError::new(
                    "pageable collection argument is absent",
                ));
            }
            if !has_positive_list_bound(&field.type_ref) {
                return Err(GraphqlSemanticError::new(
                    "semantic relationship collection is unbounded",
                ));
            }
            Ok(())
        }
        (
            GraphqlSemanticRelationshipCardinality::Many,
            Some(GraphqlSemanticCollectionBound::ServerFixed { maximum_items }),
        ) => {
            if *maximum_items == 0 {
                return Err(GraphqlSemanticError::new("semantic list bound is invalid"));
            }
            if relationship
                .arguments
                .iter()
                .any(|argument| is_declared_page_argument_name(&argument.graphql_name))
            {
                return Err(GraphqlSemanticError::new(
                    "server-fixed collection cannot declare a paging argument",
                ));
            }
            match &field.type_ref {
                GraphqlSemanticTypeRef::List {
                    maximum_items: Some(declared),
                    ..
                } if *declared == *maximum_items => Ok(()),
                _ => Err(GraphqlSemanticError::new(
                    "server-fixed collection bound does not match the list ceiling",
                )),
            }
        }
    }
}

fn is_declared_page_argument_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("page")
        || name.eq_ignore_ascii_case("pagination")
        || name.eq_ignore_ascii_case("window")
        || name.eq_ignore_ascii_case("limit")
        || name.eq_ignore_ascii_case("first")
        || name.eq_ignore_ascii_case("groupLimit")
}

fn has_positive_list_bound(type_ref: &GraphqlSemanticTypeRef) -> bool {
    match type_ref {
        GraphqlSemanticTypeRef::List { maximum_items, .. } => {
            maximum_items.is_some_and(|limit| limit > 0)
        }
        GraphqlSemanticTypeRef::Named { .. } => false,
    }
}

/// Parses a public GraphQL type signature into the closed semantic type model.
///
/// # Errors
///
/// Returns an error for malformed or unsupported type syntax.
pub fn parse_graphql_type(value: &str) -> Result<GraphqlSemanticTypeRef, GraphqlSemanticError> {
    fn parse(value: &str, nullable: bool) -> Result<GraphqlSemanticTypeRef, GraphqlSemanticError> {
        let value = value.trim();
        if let Some(inner) = value.strip_suffix('!') {
            return parse(inner, false);
        }
        if let Some(inner) = value
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        {
            return Ok(GraphqlSemanticTypeRef::list(
                nullable,
                None,
                parse(inner, true)?,
            ));
        }
        if !valid_graphql_name(value) {
            return Err(GraphqlSemanticError::new(
                "semantic GraphQL type is invalid",
            ));
        }
        let kind = match value {
            "BigInt" | "Boolean" | "Bytes" | "Date" | "DateTime" | "Decimal" | "Float" | "ID"
            | "Int" | "JSON" | "Time" | "UUID" | "Upload" | "String" => {
                GraphqlSemanticTypeKind::Scalar
            }
            _ => GraphqlSemanticTypeKind::Object,
        };
        Ok(GraphqlSemanticTypeRef::named(value, kind, nullable))
    }
    parse(value, true)
}

fn canonical_json_bytes(value: &serde_json::Value) -> Vec<u8> {
    fn canonicalize(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(object) => serde_json::Value::Object(
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), canonicalize(value)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(canonicalize).collect())
            }
            scalar => scalar.clone(),
        }
    }
    serde_json::to_vec(&canonicalize(value)).expect("canonical semantic JSON serializes")
}

fn hex_sha256(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
