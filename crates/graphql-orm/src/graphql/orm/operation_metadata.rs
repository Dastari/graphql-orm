//! Generated GraphQL resolver-operation metadata.
//!
//! [`GraphqlOperationMetadata`] describes resolver fields emitted by
//! `GraphQLOperations`. [`GraphqlOperationCatalog`] then resolves those
//! declarations against one `schema_roots!` invocation so callers can
//! distinguish generated mutations from mutations actually merged into the
//! public root.
//!
//! This module is discovery and drift-detection metadata only. It does not
//! authorize resolver execution, classify result data, bind a GraphQL document
//! projection, or replace normal resolver, row-policy, field-policy, or RLS
//! checks.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Stable identifier for the resolver-operation fingerprint algorithm.
///
/// Version 1 uses SHA-256 over a domain-separated sequence of UTF-8 fields.
/// Every field is encoded as an eight-byte big-endian label length, the label,
/// an eight-byte big-endian value length, and the value. Argument descriptors
/// are included in declaration order; catalog operations are sorted before
/// hashing. The digest is rendered as 64 lowercase hexadecimal characters.
pub const GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM: &str = "graphql-orm-sha256-len-v1";

/// Stable identifier for generated-operation authorization fingerprints.
///
/// Authorization fingerprints are deliberately separate from
/// [`GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM`]. Adding or changing a policy
/// must not silently change the established resolver-discovery fingerprint.
pub const GRAPHQL_AUTHORIZATION_FINGERPRINT_ALGORITHM: &str =
    "graphql-orm-authorization-sha256-len-v2";

/// Stable identifier for combined generated-operation router-export fingerprints.
///
/// A router-export fingerprint binds an existing discovery fingerprint to its
/// separately versioned authorization fingerprint. It describes generated ORM
/// metadata only; it is not the fingerprint of a complete protocol descriptor
/// or finished host schema.
pub const GRAPHQL_ROUTER_EXPORT_FINGERPRINT_ALGORITHM: &str =
    "graphql-orm-router-export-sha256-len-v1";

const LEGACY_SUBGRAPH_ONLY_DETAIL: &str = "no static generated-operation authorization declaration";

/// GraphQL operation root containing a generated resolver.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphqlOperationKind {
    /// A field on the generated `Query` root.
    Query,
    /// A field on the generated `Mutation` root.
    Mutation,
    /// A field on the generated `Subscription` root.
    Subscription,
}

impl GraphqlOperationKind {
    /// Returns the lowercase GraphQL operation keyword.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Mutation => "mutation",
            Self::Subscription => "subscription",
        }
    }

    /// Returns the conventional generated GraphQL root type name.
    pub const fn root_type(self) -> &'static str {
        match self {
            Self::Query => "Query",
            Self::Mutation => "Mutation",
            Self::Subscription => "Subscription",
        }
    }
}

/// Stable semantic category for a generated resolver field.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum GeneratedGraphqlOperationCategory {
    /// Offset-paginated entity connection query.
    List,
    /// Single entity lookup by its complete key.
    SingleRead,
    /// Full-text search connection query.
    Search,
    /// Keyset-paginated entity connection query.
    KeysetList,
    /// Create one entity.
    Create,
    /// Create or update one entity by a configured unique key.
    Upsert,
    /// Update one entity by key.
    Update,
    /// Update all entities matching a required non-empty filter.
    UpdateMany,
    /// Delete one entity by key.
    Delete,
    /// Delete all entities matching a required non-empty filter.
    DeleteMany,
    /// Subscribe to generated entity change events.
    Subscription,
}

impl GeneratedGraphqlOperationCategory {
    /// Returns the stable lowercase category identifier used by fingerprints.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::SingleRead => "single_read",
            Self::Search => "search",
            Self::KeysetList => "keyset_list",
            Self::Create => "create",
            Self::Upsert => "upsert",
            Self::Update => "update",
            Self::UpdateMany => "update_many",
            Self::Delete => "delete",
            Self::DeleteMany => "delete_many",
            Self::Subscription => "subscription",
        }
    }
}

/// One all-of scope set in an any-of authorization requirement.
///
/// Values may be fixed scopes or validated root-argument templates. Their
/// declaration order is not semantically meaningful and is canonicalized
/// before fingerprinting.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GraphqlAuthorizationScopeSet {
    /// Every fixed or expanded scope in this set must be granted.
    pub scopes: Vec<String>,
}

impl GraphqlAuthorizationScopeSet {
    /// Creates one all-of set from fixed scope strings.
    pub fn new<I, S>(scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            scopes: scopes.into_iter().map(Into::into).collect(),
        }
    }
}

/// Stable category for policy that cannot be represented as router permission.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum GraphqlUnrepresentablePolicyCode {
    /// Evaluation depends on request-time or application state.
    Dynamic,
    /// Evaluation uses a host-specific policy implementation.
    Custom,
    /// The policy is outside this fixed authorization model.
    Unsupported,
}

impl GraphqlUnrepresentablePolicyCode {
    /// Returns the stable lowercase category used by fingerprints and adapters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::Custom => "custom",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Explanation for an authorization decision retained by the subgraph.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GraphqlUnrepresentablePolicy {
    /// Stable reason category suitable for protocol conversion.
    pub code: GraphqlUnrepresentablePolicyCode,
    /// Short, non-secret explanation for operators.
    pub detail: String,
}

impl GraphqlUnrepresentablePolicy {
    /// Creates an unrepresentable-policy declaration.
    pub fn new(code: GraphqlUnrepresentablePolicyCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// Core-owned authorization requirement for one generated root field.
///
/// These values are descriptive inputs shared by generated resolver guards and
/// optional protocol adapters. They never replace authoritative resolver, row,
/// field, or database authorization. Scope strings may contain validated
/// `{argument}` placeholders; those declarations also bind the referenced
/// argument types into authorization fingerprint version 2.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GraphqlAuthorizationRequirement {
    /// No static generated-operation authentication or scope requirement.
    Public,
    /// An authenticated subject is required, without a fixed scope requirement.
    Authenticated,
    /// Every listed fixed or argument-templated scope is required.
    AllScopes {
        /// Unordered scopes evaluated with all-of semantics.
        scopes: Vec<String>,
    },
    /// At least one all-of fixed or argument-templated scope set must match.
    AnyScopes {
        /// Unordered OR alternatives, each containing an unordered AND set.
        alternatives: Vec<GraphqlAuthorizationScopeSet>,
    },
    /// The router must not infer permission because the subgraph owns the policy.
    SubgraphOnly {
        /// Stable explanation of the unrepresentable authoritative policy.
        policy: GraphqlUnrepresentablePolicy,
    },
}

impl Default for GraphqlAuthorizationRequirement {
    fn default() -> Self {
        Self::SubgraphOnly {
            policy: GraphqlUnrepresentablePolicy::new(
                GraphqlUnrepresentablePolicyCode::Unsupported,
                LEGACY_SUBGRAPH_ONLY_DETAIL,
            ),
        }
    }
}

impl GraphqlAuthorizationRequirement {
    fn canonicalized(&self) -> Self {
        match self {
            Self::AllScopes { scopes } => {
                let mut scopes = scopes.clone();
                scopes.sort();
                scopes.dedup();
                Self::AllScopes { scopes }
            }
            Self::AnyScopes { alternatives } => {
                let mut alternatives = alternatives.clone();
                for alternative in &mut alternatives {
                    alternative.scopes.sort();
                    alternative.scopes.dedup();
                }
                alternatives.sort();
                alternatives.dedup();
                Self::AnyScopes { alternatives }
            }
            Self::Public => Self::Public,
            Self::Authenticated => Self::Authenticated,
            Self::SubgraphOnly { policy } => Self::SubgraphOnly {
                policy: policy.clone(),
            },
        }
    }

    fn referenced_arguments(&self) -> BTreeSet<String> {
        let scopes: Vec<&str> = match self {
            Self::AllScopes { scopes } => scopes.iter().map(String::as_str).collect(),
            Self::AnyScopes { alternatives } => alternatives
                .iter()
                .flat_map(|alternative| alternative.scopes.iter().map(String::as_str))
                .collect(),
            Self::Public | Self::Authenticated | Self::SubgraphOnly { .. } => Vec::new(),
        };
        let mut references = BTreeSet::new();
        for scope in scopes {
            let mut remaining = scope;
            while let Some(start) = remaining.find('{') {
                let after_start = &remaining[start + 1..];
                let Some(end) = after_start.find('}') else {
                    break;
                };
                references.insert(after_start[..end].to_string());
                remaining = &after_start[end + 1..];
            }
        }
        references
    }
}

/// Static identity of one argument accepted by a generated resolver.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphqlOperationArgumentDescriptor {
    graphql_name: &'static str,
    rust_type: &'static str,
    graphql_type: String,
}

impl GraphqlOperationArgumentDescriptor {
    /// Constructs macro-generated argument metadata.
    ///
    /// This constructor is public only so `GraphQLOperations` expansions in
    /// downstream crates can create descriptors.
    #[doc(hidden)]
    pub fn generated(
        graphql_name: &'static str,
        rust_type: &'static str,
        graphql_type: impl Into<String>,
    ) -> Self {
        Self {
            graphql_name,
            rust_type,
            graphql_type: graphql_type.into(),
        }
    }

    /// Returns the exact configured GraphQL argument name.
    pub const fn graphql_name(&self) -> &'static str {
        self.graphql_name
    }

    /// Returns the generated Rust argument type spelling.
    pub const fn rust_type(&self) -> &'static str {
        self.rust_type
    }

    /// Returns the generated GraphQL argument type signature.
    pub fn graphql_type(&self) -> &str {
        &self.graphql_type
    }
}

/// Immutable metadata for one resolver emitted by `GraphQLOperations`.
///
/// This declaration proves that the derive emitted the resolver for the active
/// backend/entity feature profile. It deliberately does not claim that a
/// `schema_roots!` invocation merged the resolver into a public root. Use
/// [`GraphqlOperationCatalog::operations`] for resolved exposure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedGraphqlOperationDescriptor {
    entity_rust_type: &'static str,
    entity_name: &'static str,
    table_name: &'static str,
    backend: &'static str,
    kind: GraphqlOperationKind,
    category: GeneratedGraphqlOperationCategory,
    field_name: &'static str,
    arguments: Box<[GraphqlOperationArgumentDescriptor]>,
    rust_result_type: &'static str,
    graphql_result_type: String,
    schema_signature: &'static str,
    fingerprint: String,
    authorization: GraphqlAuthorizationRequirement,
    authorization_fingerprint: String,
    router_export_fingerprint: String,
}

impl GeneratedGraphqlOperationDescriptor {
    /// Constructs and fingerprints macro-generated resolver metadata.
    ///
    /// This constructor is public only so `GraphQLOperations` expansions in
    /// downstream crates can create descriptors.
    #[allow(clippy::too_many_arguments)]
    #[doc(hidden)]
    pub fn generated(
        entity_rust_type: &'static str,
        entity_name: &'static str,
        table_name: &'static str,
        backend: &'static str,
        kind: GraphqlOperationKind,
        category: GeneratedGraphqlOperationCategory,
        field_name: &'static str,
        arguments: Vec<GraphqlOperationArgumentDescriptor>,
        rust_result_type: &'static str,
        graphql_result_type: impl Into<String>,
        schema_signature: &'static str,
    ) -> Self {
        Self::generated_with_authorization(
            entity_rust_type,
            entity_name,
            table_name,
            backend,
            kind,
            category,
            field_name,
            arguments,
            rust_result_type,
            graphql_result_type,
            schema_signature,
            GraphqlAuthorizationRequirement::default(),
        )
    }

    /// Constructs macro-generated resolver metadata with static authorization.
    ///
    /// This constructor is public only so `GraphQLOperations` expansions in
    /// downstream crates can create descriptors. The authorization declaration
    /// affects only the separate authorization and router-export fingerprints;
    /// [`Self::fingerprint`] remains byte-compatible with [`Self::generated`].
    #[allow(clippy::too_many_arguments)]
    #[doc(hidden)]
    pub fn generated_with_authorization(
        entity_rust_type: &'static str,
        entity_name: &'static str,
        table_name: &'static str,
        backend: &'static str,
        kind: GraphqlOperationKind,
        category: GeneratedGraphqlOperationCategory,
        field_name: &'static str,
        arguments: Vec<GraphqlOperationArgumentDescriptor>,
        rust_result_type: &'static str,
        graphql_result_type: impl Into<String>,
        schema_signature: &'static str,
        authorization: GraphqlAuthorizationRequirement,
    ) -> Self {
        let mut descriptor = Self {
            entity_rust_type,
            entity_name,
            table_name,
            backend,
            kind,
            category,
            field_name,
            arguments: arguments.into_boxed_slice(),
            rust_result_type,
            graphql_result_type: graphql_result_type.into(),
            schema_signature,
            fingerprint: String::new(),
            authorization,
            authorization_fingerprint: String::new(),
            router_export_fingerprint: String::new(),
        };
        descriptor.fingerprint = descriptor.compute_fingerprint();
        descriptor.authorization_fingerprint = descriptor.compute_authorization_fingerprint();
        descriptor.router_export_fingerprint = descriptor.compute_router_export_fingerprint();
        descriptor
    }

    /// Returns the fully qualified Rust entity type emitted with `module_path!`.
    pub const fn entity_rust_type(&self) -> &'static str {
        self.entity_rust_type
    }

    /// Returns the generated entity identity.
    pub const fn entity_name(&self) -> &'static str {
        self.entity_name
    }

    /// Returns the configured physical table identity.
    pub const fn table_name(&self) -> &'static str {
        self.table_name
    }

    /// Returns the selected backend profile.
    pub const fn backend(&self) -> &'static str {
        self.backend
    }

    /// Returns the GraphQL operation kind.
    pub const fn kind(&self) -> GraphqlOperationKind {
        self.kind
    }

    /// Returns the stable generated resolver category.
    pub const fn category(&self) -> GeneratedGraphqlOperationCategory {
        self.category
    }

    /// Returns the conventional root type name.
    pub const fn root_type(&self) -> &'static str {
        self.kind.root_type()
    }

    /// Returns the exact configured GraphQL root field name.
    pub const fn field_name(&self) -> &'static str {
        self.field_name
    }

    /// Returns the generated resolver arguments in declaration order.
    pub fn arguments(&self) -> &[GraphqlOperationArgumentDescriptor] {
        &self.arguments
    }

    /// Returns the generated Rust result type spelling.
    pub const fn rust_result_type(&self) -> &'static str {
        self.rust_result_type
    }

    /// Returns the generated GraphQL result type signature.
    pub fn graphql_result_type(&self) -> &str {
        &self.graphql_result_type
    }

    /// Returns the canonical derive-owned schema declaration.
    ///
    /// The signature includes entity/backend configuration and field
    /// visibility/input/filter/order/search facts that affect generated
    /// resolver shapes. It is diagnostic metadata, not GraphQL SDL.
    pub const fn schema_signature(&self) -> &'static str {
        self.schema_signature
    }

    /// Returns the stable generated descriptor fingerprint.
    ///
    /// This detects drift in the generated resolver declaration. It is not a
    /// signature and does not bind schema-root exposure, host custom roots,
    /// server-authored documents, selected result fields, disclosure policy,
    /// runtime pagination configuration, or authorization decisions.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Returns the core-owned static authorization declaration.
    pub const fn authorization(&self) -> &GraphqlAuthorizationRequirement {
        &self.authorization
    }

    /// Returns the authorization-only generated descriptor fingerprint.
    ///
    /// This binds the root-field coordinate and canonical fixed authorization
    /// declaration. It intentionally excludes unrelated schema shape.
    pub fn authorization_fingerprint(&self) -> &str {
        &self.authorization_fingerprint
    }

    /// Returns the combined discovery-and-authorization export fingerprint.
    pub fn router_export_fingerprint(&self) -> &str {
        &self.router_export_fingerprint
    }

    fn compute_fingerprint(&self) -> String {
        let mut hash = FingerprintBuilder::new("graphql-orm:generated-operation:v1");
        hash.part("algorithm", GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM);
        hash.part("entity_rust_type", self.entity_rust_type);
        hash.part("entity_name", self.entity_name);
        hash.part("table_name", self.table_name);
        hash.part("backend", self.backend);
        hash.part("kind", self.kind.as_str());
        hash.part("category", self.category.as_str());
        hash.part("root_type", self.kind.root_type());
        hash.part("field_name", self.field_name);
        hash.part("argument_count", &self.arguments.len().to_string());
        for (index, argument) in self.arguments.iter().enumerate() {
            hash.part(
                &format!("argument[{index}].graphql_name"),
                argument.graphql_name,
            );
            hash.part(&format!("argument[{index}].rust_type"), argument.rust_type);
            hash.part(
                &format!("argument[{index}].graphql_type"),
                &argument.graphql_type,
            );
        }
        hash.part("rust_result_type", self.rust_result_type);
        hash.part("graphql_result_type", &self.graphql_result_type);
        hash.part("schema_signature", self.schema_signature);
        hash.finish()
    }

    fn compute_authorization_fingerprint(&self) -> String {
        let mut hash = FingerprintBuilder::new("graphql-orm:generated-authorization:v2");
        hash.part("algorithm", GRAPHQL_AUTHORIZATION_FINGERPRINT_ALGORITHM);
        hash.part("kind", self.kind.as_str());
        hash.part("field_name", self.field_name);
        let referenced_arguments = self.authorization.referenced_arguments();
        let mut arguments = self
            .arguments
            .iter()
            .filter(|argument| referenced_arguments.contains(argument.graphql_name))
            .collect::<Vec<_>>();
        arguments.sort_by_key(|argument| argument.graphql_name);
        hash.part("referenced_argument_count", &arguments.len().to_string());
        for (index, argument) in arguments.into_iter().enumerate() {
            hash.part(
                &format!("referenced_argument[{index}].graphql_name"),
                argument.graphql_name,
            );
            hash.part(
                &format!("referenced_argument[{index}].graphql_type"),
                &argument.graphql_type,
            );
            hash.part(
                &format!("referenced_argument[{index}].required"),
                if argument.graphql_type.trim_end().ends_with('!') {
                    "true"
                } else {
                    "false"
                },
            );
        }
        hash.authorization(&self.authorization);
        hash.finish()
    }

    fn compute_router_export_fingerprint(&self) -> String {
        let mut hash = FingerprintBuilder::new("graphql-orm:generated-router-export:v1");
        hash.part("algorithm", GRAPHQL_ROUTER_EXPORT_FINGERPRINT_ALGORITHM);
        hash.part("operation_fingerprint", &self.fingerprint);
        hash.part("authorization_fingerprint", &self.authorization_fingerprint);
        hash.finish()
    }
}

/// Trait implemented for generated resolver discovery.
///
/// `GraphQLOperations` returns every generated resolver declaration.
/// `RepositoryEntity` returns an empty slice because it intentionally has no
/// GraphQL surface.
pub trait GraphqlOperationMetadata {
    /// Returns all resolver declarations emitted for this entity/profile.
    ///
    /// Ordering is stable by operation kind, field, and category. The returned
    /// declarations have not yet resolved schema-root mutation exposure.
    fn generated_graphql_operations() -> &'static [GeneratedGraphqlOperationDescriptor];
}

/// Schema-root-resolved metadata for one generated resolver.
///
/// A value with [`Self::is_exposed`] equal to `false` was generated for its
/// entity but omitted by `schema_roots!` root composition. Queries are exposed
/// whenever their entity participates in the schema; generated mutations and
/// subscriptions additionally depend on mutation exposure and read-only
/// root/backend policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphqlResolverOperationDescriptor {
    generated: &'static GeneratedGraphqlOperationDescriptor,
    exposed: bool,
    fingerprint: String,
    authorization_fingerprint: String,
    router_export_fingerprint: String,
}

impl GraphqlResolverOperationDescriptor {
    fn new(generated: &'static GeneratedGraphqlOperationDescriptor, exposed: bool) -> Self {
        let mut hash = FingerprintBuilder::new("graphql-orm:resolved-operation:v1");
        hash.part("generated_fingerprint", generated.fingerprint());
        hash.part("exposed", if exposed { "true" } else { "false" });
        let fingerprint = hash.finish();

        let mut authorization_hash =
            FingerprintBuilder::new("graphql-orm:resolved-authorization:v2");
        authorization_hash.part("algorithm", GRAPHQL_AUTHORIZATION_FINGERPRINT_ALGORITHM);
        authorization_hash.part(
            "generated_authorization_fingerprint",
            generated.authorization_fingerprint(),
        );
        authorization_hash.part("exposed", if exposed { "true" } else { "false" });
        let authorization_fingerprint = authorization_hash.finish();

        let mut export_hash = FingerprintBuilder::new("graphql-orm:resolved-router-export:v1");
        export_hash.part("algorithm", GRAPHQL_ROUTER_EXPORT_FINGERPRINT_ALGORITHM);
        export_hash.part("operation_fingerprint", &fingerprint);
        export_hash.part("authorization_fingerprint", &authorization_fingerprint);
        Self {
            generated,
            exposed,
            fingerprint,
            authorization_fingerprint,
            router_export_fingerprint: export_hash.finish(),
        }
    }

    /// Returns the underlying derive-generated declaration.
    pub const fn generated(&self) -> &'static GeneratedGraphqlOperationDescriptor {
        self.generated
    }

    /// Returns whether this resolver is actually merged into the schema root.
    pub const fn is_exposed(&self) -> bool {
        self.exposed
    }

    /// Returns the GraphQL operation kind.
    pub const fn kind(&self) -> GraphqlOperationKind {
        self.generated.kind()
    }

    /// Returns the stable generated resolver category.
    pub const fn category(&self) -> GeneratedGraphqlOperationCategory {
        self.generated.category()
    }

    /// Returns the conventional root type name.
    pub const fn root_type(&self) -> &'static str {
        self.generated.root_type()
    }

    /// Returns the exact configured GraphQL root field name.
    pub const fn field_name(&self) -> &'static str {
        self.generated.field_name()
    }

    /// Returns the fully qualified Rust entity type.
    pub const fn entity_rust_type(&self) -> &'static str {
        self.generated.entity_rust_type()
    }

    /// Returns the generated entity identity.
    pub const fn entity_name(&self) -> &'static str {
        self.generated.entity_name()
    }

    /// Returns the configured physical table identity.
    pub const fn table_name(&self) -> &'static str {
        self.generated.table_name()
    }

    /// Returns the selected backend profile.
    pub const fn backend(&self) -> &'static str {
        self.generated.backend()
    }

    /// Returns the generated resolver arguments.
    pub fn arguments(&self) -> &[GraphqlOperationArgumentDescriptor] {
        self.generated.arguments()
    }

    /// Returns the generated Rust result type spelling.
    pub const fn rust_result_type(&self) -> &'static str {
        self.generated.rust_result_type()
    }

    /// Returns the generated GraphQL result type signature.
    pub fn graphql_result_type(&self) -> &str {
        self.generated.graphql_result_type()
    }

    /// Returns the exposure-resolved descriptor fingerprint.
    ///
    /// This binds the derive descriptor fingerprint and the schema-root
    /// exposure decision. It proves neither authorization nor disclosure.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Returns the generated field's core-owned authorization declaration.
    pub const fn authorization(&self) -> &GraphqlAuthorizationRequirement {
        self.generated.authorization()
    }

    /// Returns the exposure-resolved authorization fingerprint.
    pub fn authorization_fingerprint(&self) -> &str {
        &self.authorization_fingerprint
    }

    /// Returns the exposure-resolved combined router-export fingerprint.
    pub fn router_export_fingerprint(&self) -> &str {
        &self.router_export_fingerprint
    }
}

/// Deterministic catalog of generated operations for one `schema_roots!` use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphqlOperationCatalog {
    operations: Box<[GraphqlResolverOperationDescriptor]>,
    fingerprint: String,
    authorization_fingerprint: String,
    router_export_fingerprint: String,
}

impl GraphqlOperationCatalog {
    /// Composes descriptors with per-entity mutation/subscription exposure.
    ///
    /// Each iterator item contains one entity's static derive descriptors and
    /// whether that entity's generated mutation and subscription types were
    /// merged into the schema. Queries remain exposed whenever generated.
    ///
    /// This method is public because `schema_roots!` expands in downstream
    /// crates; applications normally call its generated
    /// `graphql_orm_operation_catalog()` helper instead.
    #[doc(hidden)]
    pub fn compose<I>(groups: I) -> Self
    where
        I: IntoIterator<Item = (&'static [GeneratedGraphqlOperationDescriptor], bool, bool)>,
    {
        let mut operations = groups
            .into_iter()
            .flat_map(|(generated, mutations_exposed, subscriptions_exposed)| {
                generated.iter().map(move |operation| {
                    let exposed = match operation.kind() {
                        GraphqlOperationKind::Query => true,
                        GraphqlOperationKind::Mutation => mutations_exposed,
                        GraphqlOperationKind::Subscription => subscriptions_exposed,
                    };
                    GraphqlResolverOperationDescriptor::new(operation, exposed)
                })
            })
            .collect::<Vec<_>>();
        operations.sort_by(|left, right| {
            (
                left.kind(),
                left.field_name(),
                left.entity_rust_type(),
                left.category(),
            )
                .cmp(&(
                    right.kind(),
                    right.field_name(),
                    right.entity_rust_type(),
                    right.category(),
                ))
        });

        let mut hash = FingerprintBuilder::new("graphql-orm:operation-catalog:v1");
        hash.part("algorithm", GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM);
        hash.part("operation_count", &operations.len().to_string());
        for (index, operation) in operations.iter().enumerate() {
            hash.part(
                &format!("operation[{index}].fingerprint"),
                operation.fingerprint(),
            );
        }
        let fingerprint = hash.finish();

        let mut authorization_hash =
            FingerprintBuilder::new("graphql-orm:authorization-catalog:v2");
        authorization_hash.part("algorithm", GRAPHQL_AUTHORIZATION_FINGERPRINT_ALGORITHM);
        authorization_hash.part("operation_count", &operations.len().to_string());
        for (index, operation) in operations.iter().enumerate() {
            authorization_hash.part(
                &format!("operation[{index}].authorization_fingerprint"),
                operation.authorization_fingerprint(),
            );
        }
        let authorization_fingerprint = authorization_hash.finish();

        let mut export_hash = FingerprintBuilder::new("graphql-orm:router-export-catalog:v1");
        export_hash.part("algorithm", GRAPHQL_ROUTER_EXPORT_FINGERPRINT_ALGORITHM);
        export_hash.part("operation_catalog_fingerprint", &fingerprint);
        export_hash.part("authorization_fingerprint", &authorization_fingerprint);
        Self {
            operations: operations.into_boxed_slice(),
            fingerprint,
            authorization_fingerprint,
            router_export_fingerprint: export_hash.finish(),
        }
    }

    /// Returns all generated operations, including mutations omitted at root.
    pub fn operations(&self) -> &[GraphqlResolverOperationDescriptor] {
        &self.operations
    }

    /// Iterates only operations actually exposed by this generated schema root.
    pub fn exposed_operations(&self) -> impl Iterator<Item = &GraphqlResolverOperationDescriptor> {
        self.operations
            .iter()
            .filter(|operation| operation.is_exposed())
    }

    /// Resolves one unique exposed generated root field.
    ///
    /// Returns `None` when the coordinate is absent, omitted, or ambiguous.
    /// A GraphQL document operation name is consumer-authored and is not the
    /// same identity as this generated root field coordinate.
    pub fn resolve(
        &self,
        kind: GraphqlOperationKind,
        field_name: &str,
    ) -> Option<&GraphqlResolverOperationDescriptor> {
        let mut matching = self.operations.iter().filter(|operation| {
            operation.is_exposed()
                && operation.kind() == kind
                && operation.field_name() == field_name
        });
        let operation = matching.next()?;
        matching.next().is_none().then_some(operation)
    }

    /// Returns the deterministic generated-operation catalog fingerprint.
    ///
    /// The fingerprint binds all derive descriptors, backend profiles, and
    /// schema-root mutation exposure decisions in stable order. It does not
    /// bind custom root types, the complete finished host SDL, a server-authored
    /// GraphQL document, a result projection, disclosure classification,
    /// runtime pagination limits, or authorization/RLS state. Hosts needing a
    /// complete target-schema fingerprint must combine or replace it with a
    /// fingerprint of their finished schema registry.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Returns the deterministic authorization fingerprint for this catalog.
    ///
    /// This includes every generated operation and its resolved exposure state,
    /// matching the scope of [`Self::fingerprint`].
    pub fn authorization_fingerprint(&self) -> &str {
        &self.authorization_fingerprint
    }

    /// Returns the combined generated catalog and authorization fingerprint.
    ///
    /// This is an ORM router-export input, not the fingerprint of a complete
    /// [`graphql-orm-router-protocol`](https://docs.rs/graphql-orm-router-protocol)
    /// descriptor or finished host SDL.
    pub fn router_export_fingerprint(&self) -> &str {
        &self.router_export_fingerprint
    }

    /// Converts exposed generated operations into protocol v1 declarations.
    ///
    /// This adapter is available only with the `router-protocol` feature. It
    /// exports advisory generated-operation metadata; authoritative resolver,
    /// row, field, and database policies remain in the subgraph. The caller
    /// still owns subgraph identity, endpoints, finished-schema fingerprint,
    /// and construction of the complete protocol descriptor.
    #[cfg(feature = "router-protocol")]
    pub fn router_protocol_operations(
        &self,
    ) -> Result<
        Vec<graphql_orm_router_protocol::OperationDescriptor>,
        graphql_orm_router_protocol::ProtocolError,
    > {
        self.exposed_operations()
            .map(|operation| {
                Ok(graphql_orm_router_protocol::OperationDescriptor {
                    root_type: protocol_root_type(operation.kind()),
                    field_name: operation.field_name().to_string(),
                    arguments: operation
                        .arguments()
                        .iter()
                        .map(|argument| graphql_orm_router_protocol::ArgumentDescriptor {
                            name: argument.graphql_name().to_string(),
                            graphql_type: argument.graphql_type().to_string(),
                            required: argument.graphql_type().trim_end().ends_with('!'),
                        })
                        .collect(),
                    authorization: protocol_authorization(operation.authorization())?,
                })
            })
            .collect()
    }
}

#[cfg(feature = "router-protocol")]
fn protocol_root_type(
    kind: GraphqlOperationKind,
) -> graphql_orm_router_protocol::RootOperationType {
    match kind {
        GraphqlOperationKind::Query => graphql_orm_router_protocol::RootOperationType::Query,
        GraphqlOperationKind::Mutation => graphql_orm_router_protocol::RootOperationType::Mutation,
        GraphqlOperationKind::Subscription => {
            graphql_orm_router_protocol::RootOperationType::Subscription
        }
    }
}

#[cfg(feature = "router-protocol")]
fn protocol_authorization(
    authorization: &GraphqlAuthorizationRequirement,
) -> Result<
    graphql_orm_router_protocol::AuthorizationRequirement,
    graphql_orm_router_protocol::ProtocolError,
> {
    use graphql_orm_router_protocol::{
        AuthorizationRequirement, ScopeSet, ScopeTemplate, UnrepresentablePolicy,
        UnrepresentablePolicyCode,
    };

    Ok(match authorization {
        GraphqlAuthorizationRequirement::Public => AuthorizationRequirement::Public,
        GraphqlAuthorizationRequirement::Authenticated => AuthorizationRequirement::Authenticated,
        GraphqlAuthorizationRequirement::AllScopes { scopes } => {
            AuthorizationRequirement::AllScopes {
                scopes: scopes
                    .iter()
                    .map(|scope| ScopeTemplate::parse(scope.clone()))
                    .collect::<Result<_, _>>()?,
            }
        }
        GraphqlAuthorizationRequirement::AnyScopes { alternatives } => {
            AuthorizationRequirement::AnyScopes {
                alternatives: alternatives
                    .iter()
                    .map(|alternative| {
                        Ok(ScopeSet {
                            scopes: alternative
                                .scopes
                                .iter()
                                .map(|scope| ScopeTemplate::parse(scope.clone()))
                                .collect::<Result<_, _>>()?,
                        })
                    })
                    .collect::<Result<_, graphql_orm_router_protocol::ProtocolError>>()?,
            }
        }
        GraphqlAuthorizationRequirement::SubgraphOnly { policy } => {
            AuthorizationRequirement::SubgraphOnly {
                policy: UnrepresentablePolicy {
                    code: match policy.code {
                        GraphqlUnrepresentablePolicyCode::Dynamic => {
                            UnrepresentablePolicyCode::Dynamic
                        }
                        GraphqlUnrepresentablePolicyCode::Custom => {
                            UnrepresentablePolicyCode::Custom
                        }
                        GraphqlUnrepresentablePolicyCode::Unsupported => {
                            UnrepresentablePolicyCode::Unsupported
                        }
                    },
                    detail: policy.detail.clone(),
                },
            }
        }
    })
}

struct FingerprintBuilder {
    hasher: Sha256,
}

impl FingerprintBuilder {
    fn new(domain: &str) -> Self {
        let mut builder = Self {
            hasher: Sha256::new(),
        };
        builder.part("domain", domain);
        builder
    }

    fn part(&mut self, label: &str, value: &str) {
        self.hasher.update((label.len() as u64).to_be_bytes());
        self.hasher.update(label.as_bytes());
        self.hasher.update((value.len() as u64).to_be_bytes());
        self.hasher.update(value.as_bytes());
    }

    fn authorization(&mut self, authorization: &GraphqlAuthorizationRequirement) {
        match authorization.canonicalized() {
            GraphqlAuthorizationRequirement::Public => {
                self.part("authorization.kind", "public");
            }
            GraphqlAuthorizationRequirement::Authenticated => {
                self.part("authorization.kind", "authenticated");
            }
            GraphqlAuthorizationRequirement::AllScopes { scopes } => {
                self.part("authorization.kind", "all_scopes");
                self.part("authorization.scope_count", &scopes.len().to_string());
                for (index, scope) in scopes.iter().enumerate() {
                    self.part(&format!("authorization.scope[{index}]"), scope);
                }
            }
            GraphqlAuthorizationRequirement::AnyScopes { alternatives } => {
                self.part("authorization.kind", "any_scopes");
                self.part(
                    "authorization.alternative_count",
                    &alternatives.len().to_string(),
                );
                for (alternative_index, alternative) in alternatives.iter().enumerate() {
                    self.part(
                        &format!("authorization.alternative[{alternative_index}].scope_count"),
                        &alternative.scopes.len().to_string(),
                    );
                    for (scope_index, scope) in alternative.scopes.iter().enumerate() {
                        self.part(
                            &format!(
                                "authorization.alternative[{alternative_index}].scope[{scope_index}]"
                            ),
                            scope,
                        );
                    }
                }
            }
            GraphqlAuthorizationRequirement::SubgraphOnly { policy } => {
                self.part("authorization.kind", "subgraph_only");
                self.part("authorization.policy.code", policy.code.as_str());
                self.part("authorization.policy.detail", &policy.detail);
            }
        }
    }

    fn finish(self) -> String {
        let digest = self.hasher.finalize();
        let mut encoded = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
        }
        encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_exposure_and_catalog_order_change_fingerprints_deterministically() {
        static GENERATED: std::sync::OnceLock<Box<[GeneratedGraphqlOperationDescriptor]>> =
            std::sync::OnceLock::new();
        let generated = GENERATED.get_or_init(|| {
            vec![GeneratedGraphqlOperationDescriptor::generated(
                "example::Record",
                "Record",
                "records",
                "sqlite",
                GraphqlOperationKind::Mutation,
                GeneratedGraphqlOperationCategory::Create,
                "createRecord",
                vec![GraphqlOperationArgumentDescriptor::generated(
                    "input",
                    "GraphQLCreateRecordInput",
                    "CreateRecordInput!",
                )],
                "RecordResult",
                "RecordResult!",
                "record-shape-v1",
            )]
            .into_boxed_slice()
        });

        let exposed = GraphqlOperationCatalog::compose([(generated.as_ref(), true, true)]);
        let hidden = GraphqlOperationCatalog::compose([(generated.as_ref(), false, true)]);
        assert_eq!(exposed.fingerprint().len(), 64);
        assert_ne!(exposed.fingerprint(), hidden.fingerprint());
        assert!(
            exposed
                .resolve(GraphqlOperationKind::Mutation, "createRecord")
                .is_some()
        );
        assert!(
            hidden
                .resolve(GraphqlOperationKind::Mutation, "createRecord")
                .is_none()
        );
    }

    #[test]
    fn schema_and_entity_identity_changes_invalidate_generated_fingerprints() {
        let descriptor = |entity_rust_type, schema_signature| {
            GeneratedGraphqlOperationDescriptor::generated(
                entity_rust_type,
                "Record",
                "records",
                "sqlite",
                GraphqlOperationKind::Query,
                GeneratedGraphqlOperationCategory::SingleRead,
                "record",
                vec![GraphqlOperationArgumentDescriptor::generated(
                    "id", "String", "String!",
                )],
                "Option<Record>",
                "Record",
                schema_signature,
            )
        };
        let original = descriptor("example::Record", "record-shape-v1");
        let changed_schema = descriptor("example::Record", "record-shape-v2");
        let changed_entity = descriptor("other::Record", "record-shape-v1");

        assert_ne!(original.fingerprint(), changed_schema.fingerprint());
        assert_ne!(original.fingerprint(), changed_entity.fingerprint());
    }

    fn authorized_descriptor(
        authorization: GraphqlAuthorizationRequirement,
    ) -> GeneratedGraphqlOperationDescriptor {
        GeneratedGraphqlOperationDescriptor::generated_with_authorization(
            "example::Record",
            "Record",
            "records",
            "sqlite",
            GraphqlOperationKind::Query,
            GeneratedGraphqlOperationCategory::SingleRead,
            "record",
            vec![GraphqlOperationArgumentDescriptor::generated(
                "id", "String", "String!",
            )],
            "Option<Record>",
            "Record",
            "record-shape-v1",
            authorization,
        )
    }

    #[test]
    fn authorization_does_not_change_the_established_generated_fingerprint() {
        let legacy = GeneratedGraphqlOperationDescriptor::generated(
            "example::Record",
            "Record",
            "records",
            "sqlite",
            GraphqlOperationKind::Query,
            GeneratedGraphqlOperationCategory::SingleRead,
            "record",
            vec![GraphqlOperationArgumentDescriptor::generated(
                "id", "String", "String!",
            )],
            "Option<Record>",
            "Record",
            "record-shape-v1",
        );
        let public = authorized_descriptor(GraphqlAuthorizationRequirement::Public);
        let scoped = authorized_descriptor(GraphqlAuthorizationRequirement::AllScopes {
            scopes: vec!["records.read".to_string()],
        });

        const EXISTING_FINGERPRINT: &str =
            "1f5821ce9c4366dd9bb0021215c8ce056ba484cb441726cf91e6673ed6dfbda9";
        assert_eq!(legacy.fingerprint(), EXISTING_FINGERPRINT);
        assert_eq!(public.fingerprint(), EXISTING_FINGERPRINT);
        assert_eq!(scoped.fingerprint(), EXISTING_FINGERPRINT);
        assert_ne!(
            public.authorization_fingerprint(),
            scoped.authorization_fingerprint()
        );
        assert_ne!(
            public.router_export_fingerprint(),
            scoped.router_export_fingerprint()
        );
    }

    #[test]
    fn authorization_canonicalization_ignores_scope_order_and_duplicates() {
        let first = authorized_descriptor(GraphqlAuthorizationRequirement::AnyScopes {
            alternatives: vec![
                GraphqlAuthorizationScopeSet::new(["records.write", "records.read"]),
                GraphqlAuthorizationScopeSet::new(["global.admin"]),
            ],
        });
        let permuted = authorized_descriptor(GraphqlAuthorizationRequirement::AnyScopes {
            alternatives: vec![
                GraphqlAuthorizationScopeSet::new(["global.admin", "global.admin"]),
                GraphqlAuthorizationScopeSet::new([
                    "records.read",
                    "records.write",
                    "records.read",
                ]),
            ],
        });

        assert_eq!(first.fingerprint(), permuted.fingerprint());
        assert_eq!(
            first.authorization_fingerprint(),
            permuted.authorization_fingerprint()
        );
        assert_eq!(
            first.router_export_fingerprint(),
            permuted.router_export_fingerprint()
        );
    }

    #[test]
    fn templated_authorization_fingerprint_binds_only_referenced_argument_contracts() {
        let descriptor = |id_type, unrelated_type| {
            GeneratedGraphqlOperationDescriptor::generated_with_authorization(
                "example::Record",
                "Record",
                "records",
                "sqlite",
                GraphqlOperationKind::Query,
                GeneratedGraphqlOperationCategory::SingleRead,
                "record",
                vec![
                    GraphqlOperationArgumentDescriptor::generated("id", "String", id_type),
                    GraphqlOperationArgumentDescriptor::generated(
                        "format",
                        "String",
                        unrelated_type,
                    ),
                ],
                "Option<Record>",
                "Record",
                "record-shape-v1",
                GraphqlAuthorizationRequirement::AllScopes {
                    scopes: vec!["records.{id}.read".to_string()],
                },
            )
        };
        let original = descriptor("String!", "String");
        let referenced_changed = descriptor("Int!", "String");
        let unrelated_changed = descriptor("String!", "Int");

        assert_ne!(
            original.authorization_fingerprint(),
            referenced_changed.authorization_fingerprint()
        );
        assert_eq!(
            original.authorization_fingerprint(),
            unrelated_changed.authorization_fingerprint()
        );
        assert_ne!(original.fingerprint(), unrelated_changed.fingerprint());
    }

    #[test]
    fn resolved_and_catalog_fingerprints_separate_policy_from_discovery() {
        static PUBLIC: std::sync::OnceLock<Box<[GeneratedGraphqlOperationDescriptor]>> =
            std::sync::OnceLock::new();
        static SCOPED: std::sync::OnceLock<Box<[GeneratedGraphqlOperationDescriptor]>> =
            std::sync::OnceLock::new();
        let public = PUBLIC.get_or_init(|| {
            vec![authorized_descriptor(
                GraphqlAuthorizationRequirement::Public,
            )]
            .into_boxed_slice()
        });
        let scoped = SCOPED.get_or_init(|| {
            vec![authorized_descriptor(
                GraphqlAuthorizationRequirement::AllScopes {
                    scopes: vec!["records.read".to_string()],
                },
            )]
            .into_boxed_slice()
        });

        let public_catalog = GraphqlOperationCatalog::compose([(public.as_ref(), true, true)]);
        let scoped_catalog = GraphqlOperationCatalog::compose([(scoped.as_ref(), true, true)]);
        assert_eq!(public_catalog.fingerprint(), scoped_catalog.fingerprint());
        assert_ne!(
            public_catalog.authorization_fingerprint(),
            scoped_catalog.authorization_fingerprint()
        );
        assert_ne!(
            public_catalog.router_export_fingerprint(),
            scoped_catalog.router_export_fingerprint()
        );

        let public_operation = &public_catalog.operations()[0];
        let scoped_operation = &scoped_catalog.operations()[0];
        assert_eq!(
            public_operation.fingerprint(),
            scoped_operation.fingerprint()
        );
        assert_ne!(
            public_operation.authorization_fingerprint(),
            scoped_operation.authorization_fingerprint()
        );
        assert_ne!(
            public_operation.router_export_fingerprint(),
            scoped_operation.router_export_fingerprint()
        );
    }

    #[cfg(feature = "router-protocol")]
    #[test]
    fn protocol_adapter_exports_only_exposed_operations_and_fixed_scopes() {
        use graphql_orm_router_protocol::{AuthorizationRequirement, RootOperationType};

        static GENERATED: std::sync::OnceLock<Box<[GeneratedGraphqlOperationDescriptor]>> =
            std::sync::OnceLock::new();
        let generated = GENERATED.get_or_init(|| {
            let authorization = GraphqlAuthorizationRequirement::AnyScopes {
                alternatives: vec![
                    GraphqlAuthorizationScopeSet::new(["records.read"]),
                    GraphqlAuthorizationScopeSet::new(["records.admin"]),
                ],
            };
            vec![
                authorized_descriptor(authorization.clone()),
                GeneratedGraphqlOperationDescriptor::generated_with_authorization(
                    "example::Record",
                    "Record",
                    "records",
                    "sqlite",
                    GraphqlOperationKind::Mutation,
                    GeneratedGraphqlOperationCategory::Create,
                    "createRecord",
                    Vec::new(),
                    "Record",
                    "Record!",
                    "record-shape-v1",
                    authorization,
                ),
            ]
            .into_boxed_slice()
        });

        let exposed = GraphqlOperationCatalog::compose([(generated.as_ref(), true, true)]);
        let operations = exposed.router_protocol_operations().unwrap();
        assert_eq!(operations.len(), 2);
        let query = operations
            .iter()
            .find(|operation| operation.root_type == RootOperationType::Query)
            .unwrap();
        assert_eq!(query.field_name, "record");
        assert!(query.arguments[0].required);
        let AuthorizationRequirement::AnyScopes { alternatives } = &query.authorization else {
            panic!("fixed any-of scopes should remain representable");
        };
        assert_eq!(alternatives.len(), 2);

        let hidden = GraphqlOperationCatalog::compose([(generated.as_ref(), false, false)]);
        // Generated queries remain exposed, while the generated mutation is
        // correctly omitted from protocol export.
        assert_eq!(hidden.router_protocol_operations().unwrap().len(), 1);
    }
}
