//! Backend-neutral GraphQL execution target and operation contracts.

use async_graphql::parser::{
    parse_query,
    types::{OperationType, Selection},
};
use graphql_orm_operation_catalog::{
    GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM, GRAPHQL_SEMANTIC_FINGERPRINT_ALGORITHM,
    GraphqlOperationCatalog, GraphqlOperationKind, GraphqlResolverOperationDescriptor,
    GraphqlSemanticCatalog, GraphqlSemanticOperationDescriptor,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable deployment-owned logical GraphQL execution target identifier.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GraphqlExecutionTargetId(String);

impl GraphqlExecutionTargetId {
    /// Parses a non-secret logical target ID.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError::InvalidTarget`] for an empty, overly
    /// long, or non-ASCII identifier.
    pub fn parse(value: impl Into<String>) -> Result<Self, ToolExecutionError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(ToolExecutionError::InvalidTarget);
        }
        Ok(Self(value))
    }

    /// Returns the logical identifier without resolving a destination URL.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Serializable GraphQL operation kind used by a generated resolver binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphqlGeneratedOperationKind {
    /// Generated query resolver.
    Query,
    /// Generated mutation resolver.
    Mutation,
    /// Generated subscription resolver.
    Subscription,
}

impl GraphqlGeneratedOperationKind {
    fn from_orm(kind: GraphqlOperationKind) -> Result<Self, ToolExecutionError> {
        match kind {
            GraphqlOperationKind::Query => Ok(Self::Query),
            GraphqlOperationKind::Mutation => Ok(Self::Mutation),
            GraphqlOperationKind::Subscription => Ok(Self::Subscription),
            _ => Err(ToolExecutionError::StaleContract),
        }
    }

    /// Returns the corresponding `graphql-orm` operation kind.
    pub const fn graphql_orm_kind(self) -> GraphqlOperationKind {
        match self {
            Self::Query => GraphqlOperationKind::Query,
            Self::Mutation => GraphqlOperationKind::Mutation,
            Self::Subscription => GraphqlOperationKind::Subscription,
        }
    }
}

/// Exact generated-resolver drift binding.
///
/// This value proves that one server-authored document selected one exposed
/// generated resolver in one immutable `graphql-orm` operation catalog when
/// the contract was built. It does not classify the resolver as an
/// application tool, authorize execution, bind the complete host SDL, or
/// replace result-disclosure and ordinary resolver authorization checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphqlGeneratedOperationBinding {
    fingerprint_algorithm: String,
    catalog_fingerprint: String,
    operation_fingerprint: String,
    kind: GraphqlGeneratedOperationKind,
    field_name: String,
    category: String,
}

/// Exact drift binding to one root in a finished-schema semantic catalogue.
///
/// This is descriptive evidence only. It neither admits the root as an AI
/// capability nor grants resolver, field, row, tenant, or provider authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphqlSemanticOperationBinding {
    fingerprint_algorithm: String,
    catalog_fingerprint: String,
    operation_fingerprint: String,
    kind: GraphqlGeneratedOperationKind,
    field_name: String,
}

impl GraphqlSemanticOperationBinding {
    fn new(
        catalog: &GraphqlSemanticCatalog,
        operation: &GraphqlSemanticOperationDescriptor,
    ) -> Result<Self, ToolExecutionError> {
        Ok(Self {
            fingerprint_algorithm: GRAPHQL_SEMANTIC_FINGERPRINT_ALGORITHM.to_owned(),
            catalog_fingerprint: catalog.fingerprint.clone(),
            operation_fingerprint: operation.fingerprint.clone(),
            kind: GraphqlGeneratedOperationKind::from_orm(operation.kind)?,
            field_name: operation.field_name.clone(),
        })
    }

    /// Returns the semantic fingerprint algorithm identifier.
    pub fn fingerprint_algorithm(&self) -> &str {
        &self.fingerprint_algorithm
    }

    /// Returns the exact semantic-catalogue fingerprint.
    pub fn catalog_fingerprint(&self) -> &str {
        &self.catalog_fingerprint
    }

    /// Returns the exact semantic-operation fingerprint.
    pub fn operation_fingerprint(&self) -> &str {
        &self.operation_fingerprint
    }

    /// Returns the bound root operation kind.
    pub const fn kind(&self) -> GraphqlGeneratedOperationKind {
        self.kind
    }

    /// Returns the exact public root field.
    pub fn field_name(&self) -> &str {
        &self.field_name
    }

    fn has_valid_shape(&self) -> bool {
        self.fingerprint_algorithm == GRAPHQL_SEMANTIC_FINGERPRINT_ALGORITHM
            && valid_sha256_fingerprint(&self.catalog_fingerprint)
            && valid_sha256_fingerprint(&self.operation_fingerprint)
            && valid_graphql_name(&self.field_name)
    }

    fn resolve<'a>(
        &self,
        catalog: &'a GraphqlSemanticCatalog,
        operation_name: &str,
        document: &str,
    ) -> Result<&'a GraphqlSemanticOperationDescriptor, ToolExecutionError> {
        if !self.has_valid_shape() || self.catalog_fingerprint != catalog.fingerprint {
            return Err(ToolExecutionError::StaleContract);
        }
        let operation = catalog
            .operations
            .iter()
            .find(|candidate| {
                candidate.kind == self.kind.graphql_orm_kind()
                    && candidate.field_name == self.field_name
            })
            .ok_or(ToolExecutionError::StaleContract)?;
        if operation.fingerprint != self.operation_fingerprint
            || !document_selects_exact_generated_root(
                document,
                operation_name,
                operation.kind,
                &operation.field_name,
            )
        {
            return Err(ToolExecutionError::StaleContract);
        }
        Ok(operation)
    }
}

impl GraphqlGeneratedOperationBinding {
    fn new(
        catalog: &GraphqlOperationCatalog,
        operation: &GraphqlResolverOperationDescriptor,
    ) -> Result<Self, ToolExecutionError> {
        Ok(Self {
            fingerprint_algorithm: GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM.to_owned(),
            catalog_fingerprint: catalog.fingerprint().to_owned(),
            operation_fingerprint: operation.fingerprint().to_owned(),
            kind: GraphqlGeneratedOperationKind::from_orm(operation.kind())?,
            field_name: operation.field_name().to_owned(),
            category: operation.category().as_str().to_owned(),
        })
    }

    /// Returns the upstream fingerprint algorithm identifier.
    pub fn fingerprint_algorithm(&self) -> &str {
        &self.fingerprint_algorithm
    }

    /// Returns the exact generated-operation catalog fingerprint.
    pub fn catalog_fingerprint(&self) -> &str {
        &self.catalog_fingerprint
    }

    /// Returns the exposure-resolved operation fingerprint.
    pub fn operation_fingerprint(&self) -> &str {
        &self.operation_fingerprint
    }

    /// Returns the generated operation kind.
    pub const fn kind(&self) -> GraphqlGeneratedOperationKind {
        self.kind
    }

    /// Returns the exact generated root field.
    pub fn field_name(&self) -> &str {
        &self.field_name
    }

    /// Returns the stable generated resolver category.
    pub fn category(&self) -> &str {
        &self.category
    }

    fn has_valid_shape(&self) -> bool {
        self.fingerprint_algorithm == GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM
            && valid_sha256_fingerprint(&self.catalog_fingerprint)
            && valid_sha256_fingerprint(&self.operation_fingerprint)
            && !self.field_name.is_empty()
            && self.field_name.len() <= 256
            && !self.field_name.chars().any(char::is_control)
            && !self.category.is_empty()
            && self.category.len() <= 64
            && !self.category.chars().any(char::is_control)
    }

    fn resolve<'a>(
        &self,
        catalog: &'a GraphqlOperationCatalog,
        operation_name: &str,
        document: &str,
    ) -> Result<&'a GraphqlResolverOperationDescriptor, ToolExecutionError> {
        if !self.has_valid_shape() || self.catalog_fingerprint != catalog.fingerprint() {
            return Err(ToolExecutionError::StaleContract);
        }
        let operation = catalog
            .resolve(self.kind.graphql_orm_kind(), &self.field_name)
            .ok_or(ToolExecutionError::StaleContract)?;
        if operation.fingerprint() != self.operation_fingerprint
            || operation.category().as_str() != self.category
            || !document_selects_exact_generated_root(
                document,
                operation_name,
                operation.kind(),
                operation.field_name(),
            )
        {
            return Err(ToolExecutionError::StaleContract);
        }
        Ok(operation)
    }
}

/// Exact static operation binding carried to local or remote executors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphqlOperationContract {
    /// Deployment-registered logical target.
    pub target_id: GraphqlExecutionTargetId,
    /// Exact target schema fingerprint reviewed with this operation.
    pub schema_fingerprint: String,
    /// Operation name inside the server-authored document.
    pub operation_name: String,
    /// Stable hash of the server-authored document.
    pub document_hash: String,
    /// Stable result-projection fingerprint.
    pub result_projection_fingerprint: String,
    /// Static disclosure-schema fingerprint.
    pub disclosure_schema_fingerprint: String,
    /// Optional exact binding to an exposed derive-generated resolver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_operation: Option<GraphqlGeneratedOperationBinding>,
    /// Optional exact binding to the canonical finished-schema semantic root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_operation: Option<GraphqlSemanticOperationBinding>,
}

impl GraphqlOperationContract {
    /// Binds a server-authored operation to its target/schema/projection contracts.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError::StaleContract`] for missing contract data.
    pub fn new(
        target_id: GraphqlExecutionTargetId,
        schema_fingerprint: impl Into<String>,
        operation_name: impl Into<String>,
        document: &str,
        result_projection_fingerprint: impl Into<String>,
        disclosure_schema_fingerprint: impl Into<String>,
    ) -> Result<Self, ToolExecutionError> {
        let contract = Self {
            target_id,
            schema_fingerprint: schema_fingerprint.into(),
            operation_name: operation_name.into(),
            document_hash: stable_graphql_document_hash(document),
            result_projection_fingerprint: result_projection_fingerprint.into(),
            disclosure_schema_fingerprint: disclosure_schema_fingerprint.into(),
            generated_operation: None,
            semantic_operation: None,
        };
        if contract.schema_fingerprint.trim().is_empty()
            || contract.operation_name.trim().is_empty()
            || document.trim().is_empty()
            || contract.result_projection_fingerprint.trim().is_empty()
            || contract.disclosure_schema_fingerprint.trim().is_empty()
        {
            return Err(ToolExecutionError::StaleContract);
        }
        Ok(contract)
    }

    /// Binds this contract to one exposed generated resolver and its catalog.
    ///
    /// The document must contain exactly one named operation, select exactly
    /// one unaliased root field, and match this contract's operation name and
    /// document hash. Nested selections remain subject to the host's finished
    /// schema validation and disclosure contract.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError::StaleContract`] when the generated
    /// coordinate is absent, unexposed, ambiguous, or does not exactly match
    /// the server-authored document.
    pub fn with_generated_operation(
        mut self,
        catalog: &GraphqlOperationCatalog,
        kind: GraphqlOperationKind,
        field_name: &str,
        document: &str,
    ) -> Result<Self, ToolExecutionError> {
        if self.document_hash != stable_graphql_document_hash(document) {
            return Err(ToolExecutionError::StaleContract);
        }
        let operation = catalog
            .resolve(kind, field_name)
            .ok_or(ToolExecutionError::StaleContract)?;
        let binding = GraphqlGeneratedOperationBinding::new(catalog, operation)?;
        binding.resolve(catalog, &self.operation_name, document)?;
        self.generated_operation = Some(binding);
        Ok(self)
    }

    /// Returns the generated resolver binding, when this is not a custom root.
    pub const fn generated_operation(&self) -> Option<&GraphqlGeneratedOperationBinding> {
        self.generated_operation.as_ref()
    }

    /// Binds this contract to one exact semantic-catalogue root of the
    /// expected operation kind.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError::StaleContract`] when the coordinate is
    /// absent, ambiguous, of a different kind, or the document does not
    /// select it as its sole root field.
    pub fn with_semantic_operation_kind(
        mut self,
        catalog: &GraphqlSemanticCatalog,
        kind: GraphqlOperationKind,
        field_name: &str,
        document: &str,
    ) -> Result<Self, ToolExecutionError> {
        if self.document_hash != stable_graphql_document_hash(document) {
            return Err(ToolExecutionError::StaleContract);
        }
        catalog
            .validate()
            .map_err(|_| ToolExecutionError::StaleContract)?;
        let mut matches = catalog
            .operations
            .iter()
            .filter(|operation| operation.kind == kind && operation.field_name == field_name);
        let operation = matches.next().ok_or(ToolExecutionError::StaleContract)?;
        if matches.next().is_some() {
            return Err(ToolExecutionError::StaleContract);
        }
        let binding = GraphqlSemanticOperationBinding::new(catalog, operation)?;
        binding.resolve(catalog, &self.operation_name, document)?;
        self.semantic_operation = Some(binding);
        Ok(self)
    }

    /// Binds this contract to one exact semantic-catalogue query root.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError::StaleContract`] when the coordinate is
    /// absent, ambiguous, non-query, or not the sole selected root field.
    pub fn with_semantic_operation(
        self,
        catalog: &GraphqlSemanticCatalog,
        field_name: &str,
        document: &str,
    ) -> Result<Self, ToolExecutionError> {
        self.with_semantic_operation_kind(
            catalog,
            GraphqlOperationKind::Query,
            field_name,
            document,
        )
    }

    /// Returns the semantic-root drift binding, when present.
    pub const fn semantic_operation(&self) -> Option<&GraphqlSemanticOperationBinding> {
        self.semantic_operation.as_ref()
    }

    /// Revalidates the semantic binding against the current canonical graph.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError::StaleContract`] for schema/catalogue,
    /// operation, coordinate, or document drift.
    pub fn resolve_semantic_operation<'a>(
        &self,
        catalog: &'a GraphqlSemanticCatalog,
        document: &str,
    ) -> Result<&'a GraphqlSemanticOperationDescriptor, ToolExecutionError> {
        if self.document_hash != stable_graphql_document_hash(document) {
            return Err(ToolExecutionError::StaleContract);
        }
        self.semantic_operation
            .as_ref()
            .ok_or(ToolExecutionError::StaleContract)?
            .resolve(catalog, &self.operation_name, document)
    }

    /// Revalidates a generated binding against the current immutable catalog.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError::StaleContract`] for a missing binding,
    /// catalog/fingerprint drift, a hidden or ambiguous resolver, or a
    /// document that no longer selects exactly the bound root field.
    pub fn resolve_generated_operation<'a>(
        &self,
        catalog: &'a GraphqlOperationCatalog,
        document: &str,
    ) -> Result<&'a GraphqlResolverOperationDescriptor, ToolExecutionError> {
        if self.document_hash != stable_graphql_document_hash(document) {
            return Err(ToolExecutionError::StaleContract);
        }
        self.generated_operation
            .as_ref()
            .ok_or(ToolExecutionError::StaleContract)?
            .resolve(catalog, &self.operation_name, document)
    }

    /// Returns whether the optional generated-operation drift binding is well formed.
    #[doc(hidden)]
    pub fn generated_operation_shape_is_valid(&self) -> bool {
        self.generated_operation
            .as_ref()
            .is_none_or(GraphqlGeneratedOperationBinding::has_valid_shape)
            && self
                .semantic_operation
                .as_ref()
                .is_none_or(GraphqlSemanticOperationBinding::has_valid_shape)
    }
}

/// Returns the canonical SHA-256 binding for a server-authored GraphQL document.
#[doc(hidden)]
pub fn stable_graphql_document_hash(document: &str) -> String {
    use sha2::{Digest, Sha256};

    hex::encode(Sha256::digest(document.as_bytes()))
}

fn valid_sha256_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_graphql_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first == b'_' || first.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

fn document_selects_exact_generated_root(
    document: &str,
    operation_name: &str,
    expected_kind: GraphqlOperationKind,
    expected_field: &str,
) -> bool {
    let Ok(document) = parse_query(document) else {
        return false;
    };
    let mut operations = document.operations.iter();
    let Some((Some(name), operation)) = operations.next() else {
        return false;
    };
    if operations.next().is_some()
        || name.as_str() != operation_name
        || !operation.node.directives.is_empty()
        || !matches!(
            (operation.node.ty, expected_kind),
            (OperationType::Query, GraphqlOperationKind::Query)
                | (OperationType::Mutation, GraphqlOperationKind::Mutation)
                | (
                    OperationType::Subscription,
                    GraphqlOperationKind::Subscription
                )
        )
    {
        return false;
    }
    let [selection] = operation.node.selection_set.node.items.as_slice() else {
        return false;
    };
    let Selection::Field(field) = &selection.node else {
        return false;
    };
    field.node.alias.is_none()
        && field.node.directives.is_empty()
        && field.node.name.node.as_str() == expected_field
}

/// Safe bridge error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ToolExecutionError {
    /// Principal could not be rehydrated/currently authorized.
    #[error("tool principal reauthorization failed")]
    Reauthorization,
    /// Current host tool policy denied the exact registered request.
    #[error("tool authorization denied")]
    Authorization,
    /// Host request context could not be built.
    #[error("tool request context unavailable")]
    RequestContext,
    /// Static operation no longer validates against the host schema.
    #[error("tool operation contract is stale")]
    StaleContract,
    /// Logical target is absent, malformed, or not permitted by deployment registration.
    #[error("tool GraphQL execution target is invalid")]
    InvalidTarget,
    /// Host execution failed safely.
    #[error("tool GraphQL execution failed")]
    Execution,
    /// The host transport refused a response above its reviewed byte ceiling.
    #[error("tool GraphQL result exceeded its reviewed budget")]
    ResultBudgetExceeded,
}
