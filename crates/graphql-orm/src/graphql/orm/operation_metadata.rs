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

use sha2::{Digest, Sha256};

/// Stable identifier for the resolver-operation fingerprint algorithm.
///
/// Version 1 uses SHA-256 over a domain-separated sequence of UTF-8 fields.
/// Every field is encoded as an eight-byte big-endian label length, the label,
/// an eight-byte big-endian value length, and the value. Argument descriptors
/// are included in declaration order; catalog operations are sorted before
/// hashing. The digest is rendered as 64 lowercase hexadecimal characters.
pub const GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM: &str = "graphql-orm-sha256-len-v1";

/// GraphQL operation root containing a generated resolver.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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
        };
        descriptor.fingerprint = descriptor.compute_fingerprint();
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
/// entity but omitted by the `schema_roots!` generated-mutation exposure
/// policy. Queries and generated subscriptions are exposed whenever their
/// entity operation type participates in that schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphqlResolverOperationDescriptor {
    generated: &'static GeneratedGraphqlOperationDescriptor,
    exposed: bool,
    fingerprint: String,
}

impl GraphqlResolverOperationDescriptor {
    fn new(generated: &'static GeneratedGraphqlOperationDescriptor, exposed: bool) -> Self {
        let mut hash = FingerprintBuilder::new("graphql-orm:resolved-operation:v1");
        hash.part("generated_fingerprint", generated.fingerprint());
        hash.part("exposed", if exposed { "true" } else { "false" });
        Self {
            generated,
            exposed,
            fingerprint: hash.finish(),
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
}

/// Deterministic catalog of generated operations for one `schema_roots!` use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphqlOperationCatalog {
    operations: Box<[GraphqlResolverOperationDescriptor]>,
    fingerprint: String,
}

impl GraphqlOperationCatalog {
    /// Composes generated entity descriptors with per-entity mutation exposure.
    ///
    /// Each iterator item contains one entity's static derive descriptors and
    /// whether that entity's generated mutation type was merged into the
    /// schema. Queries and subscriptions remain exposed whenever generated.
    ///
    /// This method is public because `schema_roots!` expands in downstream
    /// crates; applications normally call its generated
    /// `graphql_orm_operation_catalog()` helper instead.
    #[doc(hidden)]
    pub fn compose<I>(groups: I) -> Self
    where
        I: IntoIterator<Item = (&'static [GeneratedGraphqlOperationDescriptor], bool)>,
    {
        let mut operations = groups
            .into_iter()
            .flat_map(|(generated, mutations_exposed)| {
                generated.iter().map(move |operation| {
                    let exposed =
                        operation.kind() != GraphqlOperationKind::Mutation || mutations_exposed;
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
        Self {
            operations: operations.into_boxed_slice(),
            fingerprint: hash.finish(),
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

        let exposed = GraphqlOperationCatalog::compose([(generated.as_ref(), true)]);
        let hidden = GraphqlOperationCatalog::compose([(generated.as_ref(), false)]);
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
}
