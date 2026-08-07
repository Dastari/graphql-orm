use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    ProtocolError, ProtocolErrorKind, ProtocolVersion, SUPPORTED_PROTOCOL_VERSION,
    fingerprint::Fingerprint,
};

const KNOWN_REQUIRED_SEMANTICS: [&str; 3] = [
    "authorizationMetadata",
    "schemaFingerprints",
    "scopeTemplates",
];

/// A stable subgraph identifier controlled by the advertised service.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct SubgraphId(String);

impl SubgraphId {
    /// Returns the stable identifier text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SubgraphId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_token(&value, "subgraph id")?;
        Ok(Self(value))
    }
}

impl From<SubgraphId> for String {
    fn from(value: SubgraphId) -> Self {
        value.0
    }
}

/// A human-readable GraphQL subgraph name.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct SubgraphName(String);

impl SubgraphName {
    /// Returns the GraphQL-compatible name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SubgraphName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let mut characters = value.bytes();
        let Some(first) = characters.next() else {
            return Err("subgraph name must not be empty".to_string());
        };
        if !(first == b'_' || first.is_ascii_alphabetic())
            || !characters.all(|character| character == b'_' || character.is_ascii_alphanumeric())
        {
            return Err("subgraph name must be GraphQL-name compatible".to_string());
        }
        Ok(Self(value))
    }
}

impl From<SubgraphName> for String {
    fn from(value: SubgraphName) -> Self {
        value.0
    }
}

/// The advertised identity of a subgraph.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubgraphIdentity {
    /// Stable service identity used by registrations and status reports.
    pub id: SubgraphId,
    /// Human-readable and GraphQL-compatible subgraph name.
    pub name: SubgraphName,
}

/// An inert endpoint advertisement.
///
/// This only rejects an empty value. It intentionally does not parse, resolve,
/// normalize, connect to, or otherwise validate a URL.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct AdvertisedEndpoint(String);

impl AdvertisedEndpoint {
    /// Returns the unvalidated endpoint text advertised by the subgraph.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AdvertisedEndpoint {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err("advertised endpoint must not be empty".to_string());
        }
        Ok(Self(value))
    }
}

impl From<AdvertisedEndpoint> for String {
    fn from(value: AdvertisedEndpoint) -> Self {
        value.0
    }
}

/// Advertised GraphQL transport endpoints.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphqlEndpoints {
    /// GraphQL-over-HTTP endpoint advertisement.
    pub http: AdvertisedEndpoint,
    /// Optional GraphQL WebSocket endpoint advertisement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub websocket: Option<AdvertisedEndpoint>,
}

/// A subgraph schema endpoint advertisement.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaAdvertisement {
    /// Endpoint from which a router may retrieve the subgraph SDL.
    pub url: AdvertisedEndpoint,
}

/// Features a subgraph advertises to compatible routers.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySet {
    /// The subgraph can serve GraphQL subscriptions.
    #[serde(default)]
    pub subscriptions: bool,
    /// The descriptor includes operation authorization metadata.
    #[serde(default)]
    pub authorization_metadata: bool,
    /// The descriptor includes schema and router-export fingerprints.
    #[serde(default)]
    pub schema_fingerprints: bool,
}

/// The GraphQL root operation containing a field.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RootOperationType {
    /// GraphQL query root.
    Query,
    /// GraphQL mutation root.
    Mutation,
    /// GraphQL subscription root.
    Subscription,
}

/// One declared GraphQL root-field argument.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArgumentDescriptor {
    /// GraphQL argument name.
    pub name: String,
    /// Printed GraphQL input type, such as `ID!`.
    pub graphql_type: String,
    /// Whether the argument is non-null with no GraphQL default.
    pub required: bool,
}

/// One all-of scope alternative.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeSet {
    /// Every template in this set must match a granted scope.
    pub scopes: Vec<ScopeTemplate>,
}

/// A project-neutral scope string that may interpolate root-field arguments.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct ScopeTemplate(String);

impl ScopeTemplate {
    /// Parses a template and validates balanced `{argument}` placeholders.
    pub fn parse(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        validate_scope_template(&value).map_err(|detail| {
            ProtocolError::new(ProtocolErrorKind::InvalidScopeTemplate, detail)
        })?;
        Ok(Self(value))
    }

    /// Returns the original template text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns placeholders in their appearance order.
    pub fn referenced_arguments(&self) -> Vec<&str> {
        let mut references = Vec::new();
        let mut remaining = self.0.as_str();
        while let Some(start) = remaining.find('{') {
            let after_start = &remaining[start + 1..];
            let end = after_start.find('}').expect("validated scope template");
            references.push(&after_start[..end]);
            remaining = &after_start[end + 1..];
        }
        references
    }
}

impl TryFrom<String> for ScopeTemplate {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_scope_template(&value)?;
        Ok(Self(value))
    }
}

impl From<ScopeTemplate> for String {
    fn from(value: ScopeTemplate) -> Self {
        value.0
    }
}

/// A reason a policy deliberately remains authoritative in the subgraph only.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnrepresentablePolicy {
    /// Stable reason category.
    pub code: UnrepresentablePolicyCode,
    /// Short implementation-neutral explanation for administrators.
    pub detail: String,
}

/// Stable categories for policies not represented as router permissions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UnrepresentablePolicyCode {
    /// Evaluation depends on dynamic state not advertised by the protocol.
    Dynamic,
    /// Evaluation relies on an application-specific policy implementation.
    Custom,
    /// The policy cannot be expressed as this protocol's fixed scope semantics.
    Unsupported,
}

/// Router-readable authorization metadata for one root field.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AuthorizationRequirement {
    /// The field has no router preflight requirement.
    Public,
    /// A caller must be authenticated; scope checks are not declared.
    Authenticated,
    /// Every scope in `scopes` is required.
    AllScopes {
        /// Required scopes.
        scopes: Vec<ScopeTemplate>,
    },
    /// One all-of scope set must be satisfied.
    AnyScopes {
        /// OR alternatives; each member is an all-of scope set.
        alternatives: Vec<ScopeSet>,
    },
    /// The subgraph owns this policy and the router must not infer permission.
    SubgraphOnly {
        /// Reason router authorization is deliberately not representable.
        policy: UnrepresentablePolicy,
    },
}

/// One root field exposed by a subgraph.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationDescriptor {
    /// Root operation containing this field.
    pub root_type: RootOperationType,
    /// GraphQL root field name.
    pub field_name: String,
    /// Declared root-field arguments.
    #[serde(default)]
    pub arguments: Vec<ArgumentDescriptor>,
    /// Advisory router authorization metadata.
    pub authorization: AuthorizationRequirement,
}

/// Independently meaningful fingerprints for one subgraph advertisement.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DescriptorFingerprints {
    /// Fingerprint of the advertised subgraph schema, generated by the subgraph.
    pub schema: Fingerprint,
    /// Canonical fingerprint of declared operation authorization metadata.
    pub authorization: Fingerprint,
    /// Canonical fingerprint of router-relevant descriptor metadata.
    pub combined: Fingerprint,
}

/// A complete subgraph advertisement for protocol version 1.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubgraphDescriptor {
    /// Version of the producer's protocol payload.
    pub protocol_version: ProtocolVersion,
    /// Advertised subgraph identity.
    pub subgraph: SubgraphIdentity,
    /// Advertised GraphQL transport endpoints.
    pub graphql: GraphqlEndpoints,
    /// Advertised schema endpoint.
    pub schema: SchemaAdvertisement,
    /// Subgraph feature advertisements.
    #[serde(default)]
    pub capabilities: CapabilitySet,
    /// Semantics the producer requires this reader to understand.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_semantics: Vec<String>,
    /// Declared root operation metadata.
    #[serde(default)]
    pub operations: Vec<OperationDescriptor>,
    /// Schema, authorization, and router-export fingerprints.
    pub fingerprints: DescriptorFingerprints,
}

/// Framework-neutral constructor for a complete protocol v1 descriptor.
///
/// Host applications can serialize the built value from any HTTP framework at
/// `/.well-known/graphql-router`; this package deliberately owns no server.
#[derive(Clone, Debug)]
pub struct SubgraphDescriptorBuilder {
    descriptor: SubgraphDescriptor,
}

impl SubgraphDescriptorBuilder {
    /// Starts a v1 descriptor with stable identity and required endpoints.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        graphql_http: impl Into<String>,
        schema_url: impl Into<String>,
        schema_fingerprint: Fingerprint,
    ) -> Result<Self, ProtocolError> {
        let id = SubgraphId::try_from(id.into())
            .map_err(|detail| ProtocolError::new(ProtocolErrorKind::InvalidDescriptor, detail))?;
        let name = SubgraphName::try_from(name.into())
            .map_err(|detail| ProtocolError::new(ProtocolErrorKind::InvalidDescriptor, detail))?;
        let endpoint = |value: String| {
            AdvertisedEndpoint::try_from(value)
                .map_err(|detail| ProtocolError::new(ProtocolErrorKind::InvalidDescriptor, detail))
        };
        Ok(Self {
            descriptor: SubgraphDescriptor {
                protocol_version: SUPPORTED_PROTOCOL_VERSION,
                subgraph: SubgraphIdentity { id, name },
                graphql: GraphqlEndpoints {
                    http: endpoint(graphql_http.into())?,
                    websocket: None,
                },
                schema: SchemaAdvertisement {
                    url: endpoint(schema_url.into())?,
                },
                capabilities: CapabilitySet::default(),
                required_semantics: Vec::new(),
                operations: Vec::new(),
                fingerprints: DescriptorFingerprints {
                    schema: schema_fingerprint,
                    authorization: Fingerprint::sha256("pending authorization"),
                    combined: Fingerprint::sha256("pending combined"),
                },
            },
        })
    }

    /// Advertises the optional GraphQL WebSocket destination.
    pub fn websocket(mut self, endpoint: impl Into<String>) -> Result<Self, ProtocolError> {
        self.descriptor.graphql.websocket = Some(
            AdvertisedEndpoint::try_from(endpoint.into()).map_err(|detail| {
                ProtocolError::new(ProtocolErrorKind::InvalidDescriptor, detail)
            })?,
        );
        Ok(self)
    }

    /// Replaces the advertised capability set.
    #[must_use]
    pub fn capabilities(mut self, capabilities: CapabilitySet) -> Self {
        self.descriptor.capabilities = capabilities;
        self
    }

    /// Adds one required semantic understood by compatible readers.
    #[must_use]
    pub fn require_semantic(mut self, semantic: impl Into<String>) -> Self {
        self.descriptor.required_semantics.push(semantic.into());
        self
    }

    /// Adds one root operation declaration.
    #[must_use]
    pub fn operation(mut self, operation: OperationDescriptor) -> Self {
        self.descriptor.operations.push(operation);
        self
    }

    /// Canonicalizes, fingerprints, and validates the complete descriptor.
    pub fn build(mut self) -> Result<SubgraphDescriptor, ProtocolError> {
        self.descriptor.canonicalize();
        self.descriptor.fingerprints.authorization = self.descriptor.authorization_fingerprint();
        self.descriptor.fingerprints.combined = self.descriptor.combined_fingerprint();
        self.descriptor.validate_compatible()?;
        Ok(self.descriptor)
    }
}

impl SubgraphDescriptor {
    /// Decodes and validates a descriptor against this crate's supported version.
    pub fn from_json_compatible(json: &str) -> Result<Self, ProtocolError> {
        let descriptor: Self = serde_json::from_str(json).map_err(|error| {
            ProtocolError::new(ProtocolErrorKind::MalformedPayload, error.to_string())
        })?;
        descriptor.validate_compatible()?;
        Ok(descriptor)
    }

    /// Validates version compatibility and all protocol v1 invariants.
    pub fn validate_compatible(&self) -> Result<(), ProtocolError> {
        self.protocol_version
            .ensure_compatible_with(SUPPORTED_PROTOCOL_VERSION)?;
        self.validate_required_semantics()?;
        self.validate_operations()?;
        self.validate_fingerprints()
    }

    /// Sorts unordered logical collections into their canonical protocol order.
    pub fn canonicalize(&mut self) {
        self.required_semantics.sort();
        self.required_semantics.dedup();
        self.operations.sort_by(operation_order);
        for operation in &mut self.operations {
            operation
                .arguments
                .sort_by(|left, right| left.name.cmp(&right.name));
            canonicalize_authorization(&mut operation.authorization);
        }
    }

    /// Returns the canonical authorization fingerprint for `operations`.
    pub fn authorization_fingerprint(&self) -> Fingerprint {
        let mut operations = self.operations.clone();
        operations.sort_by(operation_order);
        for operation in &mut operations {
            operation
                .arguments
                .sort_by(|left, right| left.name.cmp(&right.name));
            canonicalize_authorization(&mut operation.authorization);
        }
        fingerprint_json(
            &operations
                .iter()
                .map(|operation| AuthorizationInput {
                    root_type: operation.root_type,
                    field_name: &operation.field_name,
                    arguments: referenced_template_arguments(operation),
                    authorization: &operation.authorization,
                })
                .collect::<Vec<_>>(),
        )
    }

    /// Returns the canonical fingerprint of router-relevant descriptor metadata.
    pub fn combined_fingerprint(&self) -> Fingerprint {
        let mut canonical = self.clone();
        canonical.canonicalize();
        fingerprint_json(&CombinedInput {
            protocol_major: canonical.protocol_version.major,
            subgraph: &canonical.subgraph,
            graphql: &canonical.graphql,
            schema: &canonical.schema,
            capabilities: &canonical.capabilities,
            required_semantics: &canonical.required_semantics,
            operations: &canonical.operations,
            schema_fingerprint: &canonical.fingerprints.schema,
            authorization_fingerprint: &canonical.authorization_fingerprint(),
        })
    }

    fn validate_required_semantics(&self) -> Result<(), ProtocolError> {
        for semantic in &self.required_semantics {
            if !KNOWN_REQUIRED_SEMANTICS.contains(&semantic.as_str()) {
                return Err(ProtocolError::new(
                    ProtocolErrorKind::UnknownRequiredSemantics,
                    format!("descriptor requires unsupported semantic {semantic:?}"),
                ));
            }
        }
        Ok(())
    }

    fn validate_operations(&self) -> Result<(), ProtocolError> {
        let mut operations = BTreeSet::new();
        for operation in &self.operations {
            validate_graphql_name(&operation.field_name, "operation field name")?;
            let key = (operation.root_type, operation.field_name.as_str());
            if !operations.insert(key) {
                return Err(ProtocolError::new(
                    ProtocolErrorKind::DuplicateOperation,
                    format!(
                        "duplicate {} root field {}",
                        root_name(operation.root_type),
                        operation.field_name
                    ),
                ));
            }

            let mut argument_names = BTreeSet::new();
            for argument in &operation.arguments {
                validate_graphql_name(&argument.name, "argument name")?;
                if argument.graphql_type.trim().is_empty() {
                    return Err(ProtocolError::new(
                        ProtocolErrorKind::InvalidDescriptor,
                        format!("argument {} has an empty GraphQL type", argument.name),
                    ));
                }
                if !argument_names.insert(argument.name.as_str()) {
                    return Err(ProtocolError::new(
                        ProtocolErrorKind::InvalidDescriptor,
                        format!(
                            "operation {} repeats argument {}",
                            operation.field_name, argument.name
                        ),
                    ));
                }
            }
            validate_authorization(&operation.authorization, &argument_names)?;
        }
        Ok(())
    }

    fn validate_fingerprints(&self) -> Result<(), ProtocolError> {
        let authorization = self.authorization_fingerprint();
        if self.fingerprints.authorization != authorization {
            return Err(ProtocolError::new(
                ProtocolErrorKind::FingerprintMismatch,
                "authorization fingerprint does not match canonical operation metadata",
            ));
        }
        let combined = self.combined_fingerprint();
        if self.fingerprints.combined != combined {
            return Err(ProtocolError::new(
                ProtocolErrorKind::FingerprintMismatch,
                "combined fingerprint does not match canonical router metadata",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthorizationInput<'a> {
    root_type: RootOperationType,
    field_name: &'a str,
    /// Only declarations referenced by a scope placeholder affect the
    /// authorization contract. Other argument drift belongs to the schema and
    /// combined fingerprints.
    arguments: Vec<&'a ArgumentDescriptor>,
    authorization: &'a AuthorizationRequirement,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CombinedInput<'a> {
    protocol_major: u16,
    subgraph: &'a SubgraphIdentity,
    graphql: &'a GraphqlEndpoints,
    schema: &'a SchemaAdvertisement,
    capabilities: &'a CapabilitySet,
    required_semantics: &'a [String],
    operations: &'a [OperationDescriptor],
    schema_fingerprint: &'a Fingerprint,
    authorization_fingerprint: &'a Fingerprint,
}

fn fingerprint_json(value: &impl Serialize) -> Fingerprint {
    let bytes = serde_json::to_vec(value).expect("protocol fingerprint inputs always serialize");
    Fingerprint::sha256(bytes)
}

fn canonicalize_authorization(authorization: &mut AuthorizationRequirement) {
    match authorization {
        AuthorizationRequirement::AllScopes { scopes } => {
            scopes.sort();
            scopes.dedup();
        }
        AuthorizationRequirement::AnyScopes { alternatives } => {
            for alternative in alternatives.iter_mut() {
                alternative.scopes.sort();
                alternative.scopes.dedup();
            }
            alternatives.sort();
            alternatives.dedup();
        }
        AuthorizationRequirement::Public
        | AuthorizationRequirement::Authenticated
        | AuthorizationRequirement::SubgraphOnly { .. } => {}
    }
}

fn referenced_template_arguments(operation: &OperationDescriptor) -> Vec<&ArgumentDescriptor> {
    let mut referenced = BTreeSet::new();
    match &operation.authorization {
        AuthorizationRequirement::AllScopes { scopes } => {
            for scope in scopes {
                referenced.extend(scope.referenced_arguments());
            }
        }
        AuthorizationRequirement::AnyScopes { alternatives } => {
            for scope in alternatives
                .iter()
                .flat_map(|alternative| &alternative.scopes)
            {
                referenced.extend(scope.referenced_arguments());
            }
        }
        AuthorizationRequirement::Public
        | AuthorizationRequirement::Authenticated
        | AuthorizationRequirement::SubgraphOnly { .. } => {}
    }
    operation
        .arguments
        .iter()
        .filter(|argument| referenced.contains(argument.name.as_str()))
        .collect()
}

fn validate_authorization(
    authorization: &AuthorizationRequirement,
    arguments: &BTreeSet<&str>,
) -> Result<(), ProtocolError> {
    let scopes: Vec<&ScopeTemplate> = match authorization {
        AuthorizationRequirement::AllScopes { scopes } => {
            if scopes.is_empty() {
                return Err(ProtocolError::new(
                    ProtocolErrorKind::InvalidDescriptor,
                    "allScopes must contain at least one scope",
                ));
            }
            scopes.iter().collect()
        }
        AuthorizationRequirement::AnyScopes { alternatives } => {
            if alternatives.is_empty()
                || alternatives
                    .iter()
                    .any(|alternative| alternative.scopes.is_empty())
            {
                return Err(ProtocolError::new(
                    ProtocolErrorKind::InvalidDescriptor,
                    "anyScopes must contain non-empty alternatives",
                ));
            }
            alternatives
                .iter()
                .flat_map(|alternative| alternative.scopes.iter())
                .collect()
        }
        AuthorizationRequirement::SubgraphOnly { policy } if policy.detail.trim().is_empty() => {
            return Err(ProtocolError::new(
                ProtocolErrorKind::InvalidDescriptor,
                "subgraphOnly policy detail must not be empty",
            ));
        }
        AuthorizationRequirement::Public
        | AuthorizationRequirement::Authenticated
        | AuthorizationRequirement::SubgraphOnly { .. } => Vec::new(),
    };

    for scope in scopes {
        for argument in scope.referenced_arguments() {
            if !arguments.contains(argument) {
                return Err(ProtocolError::new(
                    ProtocolErrorKind::UnknownTemplateArgument,
                    format!(
                        "scope template {:?} references unknown argument {:?}",
                        scope.as_str(),
                        argument
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn operation_order(left: &OperationDescriptor, right: &OperationDescriptor) -> std::cmp::Ordering {
    (left.root_type, &left.field_name).cmp(&(right.root_type, &right.field_name))
}

fn root_name(root: RootOperationType) -> &'static str {
    match root {
        RootOperationType::Query => "query",
        RootOperationType::Mutation => "mutation",
        RootOperationType::Subscription => "subscription",
    }
}

fn validate_token(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || !value.bytes().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, b'-' | b'_' | b'.')
        })
    {
        return Err(format!(
            "{label} must contain only ASCII letters, digits, '-', '_' or '.'"
        ));
    }
    Ok(())
}

fn validate_graphql_name(value: &str, label: &str) -> Result<(), ProtocolError> {
    SubgraphName::try_from(value.to_string())
        .map(|_| ())
        .map_err(|detail| {
            ProtocolError::new(
                ProtocolErrorKind::InvalidDescriptor,
                format!("{label}: {detail}"),
            )
        })
}

fn validate_scope_template(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("scope template must not be empty".to_string());
    }
    let mut remaining = value;
    while let Some(start) = remaining.find('{') {
        if remaining[..start].contains('}') {
            return Err("scope template has an unmatched closing brace".to_string());
        }
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find('}') else {
            return Err("scope template has an unmatched opening brace".to_string());
        };
        let argument = &after_start[..end];
        SubgraphName::try_from(argument.to_string()).map_err(|_| {
            "scope template placeholders must contain GraphQL argument names".to_string()
        })?;
        remaining = &after_start[end + 1..];
    }
    if remaining.contains('}') {
        return Err("scope template has an unmatched closing brace".to_string());
    }
    Ok(())
}
