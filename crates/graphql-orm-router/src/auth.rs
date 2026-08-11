use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
    time::{Duration, SystemTime},
};

use futures::future::BoxFuture;
use graphql_orm_router_protocol::{
    ArgumentDescriptor, AuthorizationRequirement, OperationDescriptor, RootOperationType,
    ScopeTemplate, SubgraphDescriptor,
};
use hive_router::{
    query_planner::{
        ast::{
            operation::OperationDefinition, selection_item::SelectionItem,
            selection_set::FieldSelection, value::Value,
        },
        state::supergraph_state::{OperationKind, SupergraphDefinition},
    },
    sonic_rs::{JsonValueTrait, Value as JsonValue},
};

use crate::{RouterError, RouterErrorKind, federation::ActiveGraph};

type RootFieldKey = (RootOperationType, String);
type RootArgumentContract = BTreeMap<String, String>;
type GraphRootContract = BTreeMap<RootFieldKey, RootArgumentContract>;

/// Authenticated, resource-server-owned identity available to router policy.
#[derive(Clone, Eq, PartialEq)]
pub struct AuthenticatedPrincipal {
    subject: String,
    scopes: Arc<[String]>,
    expires_at: Option<SystemTime>,
}

impl AuthenticatedPrincipal {
    /// Creates a validated principal. Providers must not construct this before
    /// all signature, issuer, audience, and time checks succeed.
    pub fn new(
        subject: impl Into<String>,
        scopes: impl IntoIterator<Item = impl Into<String>>,
        expires_at: Option<SystemTime>,
    ) -> Result<Self, AuthenticationError> {
        let subject = subject.into();
        if subject.trim().is_empty() {
            return Err(AuthenticationError::invalid_credential(
                "authenticated subject is empty",
            ));
        }
        let mut scopes = scopes.into_iter().map(Into::into).collect::<Vec<_>>();
        if scopes.iter().any(|scope| {
            scope.is_empty()
                || scope
                    .bytes()
                    .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        }) {
            return Err(AuthenticationError::invalid_credential(
                "authenticated scope set contains an invalid value",
            ));
        }
        scopes.sort();
        scopes.dedup();
        Ok(Self {
            subject,
            scopes: scopes.into(),
            expires_at,
        })
    }

    /// Stable subject identifier from the validated credential.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Validated exact scope strings.
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    /// Validated credential expiry when supplied by the provider.
    pub fn expires_at(&self) -> Option<SystemTime> {
        self.expires_at
    }
}

impl fmt::Debug for AuthenticatedPrincipal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedPrincipal")
            .field("subject", &self.subject)
            .field("scope_count", &self.scopes.len())
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Stable authentication provider failure category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthenticationErrorKind {
    /// The credential is malformed, unverifiable, expired, or otherwise invalid.
    InvalidCredential,
    /// Required key or validation infrastructure is temporarily unavailable.
    Unavailable,
}

/// Redacted authentication failure returned by a provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticationError {
    kind: AuthenticationErrorKind,
    detail: String,
}

impl AuthenticationError {
    /// Creates an invalid-credential error without retaining token material.
    pub fn invalid_credential(detail: impl Into<String>) -> Self {
        Self {
            kind: AuthenticationErrorKind::InvalidCredential,
            detail: detail.into(),
        }
    }

    /// Creates an unavailable-provider error.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            kind: AuthenticationErrorKind::Unavailable,
            detail: detail.into(),
        }
    }

    /// Returns the stable category.
    pub fn kind(&self) -> AuthenticationErrorKind {
        self.kind
    }

    /// Returns redacted operational detail.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for AuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for AuthenticationError {}

/// Engine-neutral resource-server authentication boundary.
pub trait AuthenticationProvider: Send + Sync + fmt::Debug + 'static {
    /// Loads initial verification state before the router can become ready.
    fn initialize(&self) -> BoxFuture<'_, Result<(), AuthenticationError>> {
        Box::pin(async { Ok(()) })
    }

    /// Validates one raw bearer token and returns its principal.
    fn authenticate_bearer(
        &self,
        token: &str,
    ) -> Result<AuthenticatedPrincipal, AuthenticationError>;

    /// Returns the provider's authentication clock.
    ///
    /// Long-lived transports use the same clock as credential validation so a
    /// connection closes at the validated token expiry without accepting an
    /// in-place refresh.
    fn current_time(&self) -> SystemTime {
        SystemTime::now()
    }

    /// Periodic refresh interval for remote verification state.
    fn refresh_interval(&self) -> Option<Duration> {
        None
    }

    /// Refreshes remote verification state. Providers without remote state use
    /// the default no-op implementation.
    fn refresh(&self) -> BoxFuture<'_, Result<(), AuthenticationError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Matching boundary shared by router preflight and subgraph policy adapters.
pub trait ScopeMatcher: Send + Sync + fmt::Debug + 'static {
    /// Returns whether one granted scope satisfies one required scope.
    fn matches(&self, granted: &str, required: &str) -> bool;
}

/// Exact, case-sensitive scope matching used by default.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExactScopeMatcher;

impl ScopeMatcher for ExactScopeMatcher {
    fn matches(&self, granted: &str, required: &str) -> bool {
        granted == required
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AuthorizationCatalog {
    graph_fingerprint: String,
    operations: BTreeMap<(RootOperationType, String), OperationDescriptor>,
}

impl AuthorizationCatalog {
    pub(crate) fn build(
        graph: &ActiveGraph,
        descriptors: &[SubgraphDescriptor],
    ) -> Result<Self, RouterError> {
        let expected = graph_root_contract(graph)?;
        let mut operations = BTreeMap::new();
        for descriptor in descriptors {
            descriptor.validate_compatible().map_err(|error| {
                RouterError::new(
                    RouterErrorKind::AuthorizationMetadata,
                    format!(
                        "authorization descriptor for subgraph `{}` is invalid: {error}",
                        descriptor.subgraph.name.as_str()
                    ),
                )
            })?;
            if !descriptor.capabilities.authorization_metadata {
                return Err(RouterError::new(
                    RouterErrorKind::AuthorizationMetadata,
                    format!(
                        "subgraph `{}` does not advertise authorization metadata",
                        descriptor.subgraph.name.as_str()
                    ),
                ));
            }
            for operation in &descriptor.operations {
                validate_template_argument_contract(operation)?;
                let key = (operation.root_type, operation.field_name.clone());
                let Some(expected_arguments) = expected.get(&key) else {
                    return Err(RouterError::new(
                        RouterErrorKind::AuthorizationMetadata,
                        format!(
                            "authorization metadata references absent field {:?}.{}",
                            key.0, key.1
                        ),
                    ));
                };
                validate_operation_argument_contract(operation, expected_arguments)?;
                if operations.insert(key.clone(), operation.clone()).is_some() {
                    return Err(RouterError::new(
                        RouterErrorKind::AuthorizationMetadata,
                        format!(
                            "authorization metadata is ambiguous for {:?}.{}",
                            key.0, key.1
                        ),
                    ));
                }
            }
        }
        let expected_keys = expected.keys().cloned().collect::<BTreeSet<_>>();
        let actual = operations.keys().cloned().collect::<BTreeSet<_>>();
        if actual != expected_keys {
            let missing = expected_keys.difference(&actual).next();
            let extra = actual.difference(&expected_keys).next();
            let details = match (missing, extra) {
                (Some((root, field)), _) => {
                    format!("authorization metadata is missing for {root:?}.{field}")
                }
                (_, Some((root, field))) => {
                    format!("authorization metadata references absent field {root:?}.{field}")
                }
                _ => "authorization metadata does not match the active graph".to_owned(),
            };
            return Err(RouterError::new(
                RouterErrorKind::AuthorizationMetadata,
                details,
            ));
        }
        Ok(Self {
            graph_fingerprint: graph.fingerprint.clone(),
            operations,
        })
    }

    pub(crate) fn ensure_bound_to(&self, graph: &ActiveGraph) -> Result<(), RouterError> {
        if self.graph_fingerprint != graph.fingerprint {
            return Err(RouterError::new(
                RouterErrorKind::AuthorizationMetadata,
                "authorization metadata is not bound to the selected active graph",
            ));
        }
        Ok(())
    }

    pub(crate) fn authorize_operation(
        &self,
        operation: &OperationDefinition,
        variables: &std::collections::HashMap<String, JsonValue>,
        principal: Option<&AuthenticatedPrincipal>,
        matcher: &dyn ScopeMatcher,
        variable_resolution: VariableResolution,
    ) -> Vec<AuthorizationDenial> {
        let mut effective_variables = variables.clone();
        if let Some(definitions) = &operation.variable_definitions {
            for definition in definitions {
                if let Some(default) = &definition.default_value {
                    effective_variables
                        .entry(definition.name.clone())
                        .or_insert_with(|| default.into());
                }
            }
        }
        let root_type = match operation.operation_kind {
            None | Some(OperationKind::Query) => RootOperationType::Query,
            Some(OperationKind::Mutation) => RootOperationType::Mutation,
            Some(OperationKind::Subscription) => RootOperationType::Subscription,
        };
        let mut fields = Vec::new();
        collect_included_root_fields(
            &operation.selection_set.items,
            &effective_variables,
            variable_resolution,
            &mut fields,
        );
        fields
            .into_iter()
            .filter_map(|field| {
                let operation = self.operations.get(&(root_type, field.name.clone()))?;
                match authorize_field(
                    operation,
                    field,
                    &effective_variables,
                    principal,
                    matcher,
                    variable_resolution,
                ) {
                    AuthorizationDecision::Allowed | AuthorizationDecision::Deferred => None,
                    AuthorizationDecision::Denied(denial) => Some(denial),
                }
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VariableResolution {
    Preflight,
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthorizationDenial {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

enum AuthorizationDecision {
    Allowed,
    Deferred,
    Denied(AuthorizationDenial),
}

fn authorize_field(
    operation: &OperationDescriptor,
    field: &FieldSelection,
    variables: &std::collections::HashMap<String, JsonValue>,
    principal: Option<&AuthenticatedPrincipal>,
    matcher: &dyn ScopeMatcher,
    variable_resolution: VariableResolution,
) -> AuthorizationDecision {
    let alternatives = match &operation.authorization {
        AuthorizationRequirement::Public | AuthorizationRequirement::SubgraphOnly { .. } => {
            return AuthorizationDecision::Allowed;
        }
        AuthorizationRequirement::Authenticated => {
            return if principal.is_some() {
                AuthorizationDecision::Allowed
            } else {
                AuthorizationDecision::Denied(AuthorizationDenial {
                    code: "UNAUTHENTICATED",
                    message: "authentication is required",
                })
            };
        }
        AuthorizationRequirement::AllScopes { scopes } => vec![scopes.as_slice()],
        AuthorizationRequirement::AnyScopes { alternatives } => alternatives
            .iter()
            .map(|alternative| alternative.scopes.as_slice())
            .collect(),
    };
    let Some(principal) = principal else {
        return AuthorizationDecision::Denied(AuthorizationDenial {
            code: "UNAUTHENTICATED",
            message: "authentication is required",
        });
    };

    let mut has_deferred_alternative = false;
    for alternative in alternatives {
        let mut alternative_is_deferred = false;
        let mut alternative_is_denied = false;
        for template in alternative {
            match render_template(template, operation, field, variables) {
                Some(required)
                    if principal
                        .scopes()
                        .iter()
                        .any(|granted| matcher.matches(granted, &required)) => {}
                Some(_) => {
                    alternative_is_denied = true;
                    break;
                }
                None if variable_resolution == VariableResolution::Preflight
                    && template_references_variable(template, field) =>
                {
                    alternative_is_deferred = true;
                }
                None => {
                    alternative_is_denied = true;
                    break;
                }
            }
        }
        if !alternative_is_denied && !alternative_is_deferred {
            return AuthorizationDecision::Allowed;
        }
        if !alternative_is_denied && alternative_is_deferred {
            has_deferred_alternative = true;
        }
    }

    if has_deferred_alternative {
        AuthorizationDecision::Deferred
    } else {
        AuthorizationDecision::Denied(AuthorizationDenial {
            code: "FORBIDDEN",
            message: "required scope is missing",
        })
    }
}

fn template_references_variable(template: &ScopeTemplate, field: &FieldSelection) -> bool {
    template.referenced_arguments().into_iter().any(|name| {
        field
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get_argument(name))
            .is_some_and(|value| matches!(value, Value::Variable(_)))
    })
}

fn render_template(
    template: &ScopeTemplate,
    operation: &OperationDescriptor,
    field: &FieldSelection,
    variables: &std::collections::HashMap<String, JsonValue>,
) -> Option<String> {
    let mut rendered = String::new();
    let mut remaining = template.as_str();
    while let Some(start) = remaining.find('{') {
        rendered.push_str(&remaining[..start]);
        let after = &remaining[start + 1..];
        let end = after.find('}')?;
        let name = &after[..end];
        let descriptor = operation
            .arguments
            .iter()
            .find(|argument| argument.name == name)?;
        let value = field.arguments.as_ref()?.get_argument(name)?;
        rendered.push_str(&canonical_scalar(value, descriptor, variables)?);
        remaining = &after[end + 1..];
    }
    if remaining.contains('}') {
        return None;
    }
    rendered.push_str(remaining);
    Some(rendered)
}

fn canonical_scalar(
    value: &Value,
    descriptor: &ArgumentDescriptor,
    variables: &std::collections::HashMap<String, JsonValue>,
) -> Option<String> {
    match value {
        Value::Variable(name) => {
            canonical_json_scalar(variables.get(name.trim_start_matches('$'))?, descriptor)
        }
        Value::String(value) | Value::Enum(value) => {
            scalar_accepts_string(descriptor).then(|| value.clone())
        }
        Value::Boolean(value) => scalar_accepts_boolean(descriptor).then(|| value.to_string()),
        Value::Int(value) => (scalar_accepts_integer(descriptor) || scalar_accepts_id(descriptor))
            .then(|| value.to_string()),
        Value::Float(value) => scalar_accepts_float(descriptor).then(|| value.to_string()),
        Value::Null | Value::List(_) | Value::Object(_) => None,
    }
}

fn canonical_json_scalar(value: &JsonValue, descriptor: &ArgumentDescriptor) -> Option<String> {
    if scalar_accepts_string(descriptor) {
        return value.as_str().map(str::to_owned).or_else(|| {
            scalar_accepts_id(descriptor)
                .then(|| canonical_json_integer(value))
                .flatten()
        });
    }
    if scalar_accepts_boolean(descriptor) {
        return value.as_bool().map(|value| value.to_string());
    }
    if scalar_accepts_integer(descriptor) {
        return canonical_json_integer(value);
    }
    if scalar_accepts_float(descriptor) {
        return value.as_f64().map(|value| value.to_string());
    }
    None
}

fn canonical_json_integer(value: &JsonValue) -> Option<String> {
    value
        .as_i64()
        .map(|value| value.to_string())
        .or_else(|| value.as_u64().map(|value| value.to_string()))
}

fn scalar_name(argument: &ArgumentDescriptor) -> &str {
    argument.graphql_type.trim_end_matches('!')
}

fn scalar_accepts_string(argument: &ArgumentDescriptor) -> bool {
    matches!(scalar_name(argument), "String" | "ID" | "UUID" | "Uuid")
}

fn scalar_accepts_id(argument: &ArgumentDescriptor) -> bool {
    scalar_name(argument) == "ID"
}

fn scalar_accepts_boolean(argument: &ArgumentDescriptor) -> bool {
    scalar_name(argument) == "Boolean"
}

fn scalar_accepts_integer(argument: &ArgumentDescriptor) -> bool {
    matches!(
        scalar_name(argument),
        "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
    )
}

fn scalar_accepts_float(argument: &ArgumentDescriptor) -> bool {
    matches!(scalar_name(argument), "Float" | "Float32" | "Float64")
}

fn validate_template_argument_contract(operation: &OperationDescriptor) -> Result<(), RouterError> {
    let templates = match &operation.authorization {
        AuthorizationRequirement::AllScopes { scopes } => scopes.iter().collect::<Vec<_>>(),
        AuthorizationRequirement::AnyScopes { alternatives } => alternatives
            .iter()
            .flat_map(|alternative| alternative.scopes.iter())
            .collect(),
        _ => return Ok(()),
    };
    for template in templates {
        for name in template.referenced_arguments() {
            let Some(argument) = operation
                .arguments
                .iter()
                .find(|argument| argument.name == name)
            else {
                return Err(RouterError::new(
                    RouterErrorKind::AuthorizationMetadata,
                    format!("scope template references unknown argument `{name}`"),
                ));
            };
            if !argument.required
                || !(scalar_accepts_string(argument)
                    || scalar_accepts_boolean(argument)
                    || scalar_accepts_integer(argument)
                    || scalar_accepts_float(argument))
            {
                return Err(RouterError::new(
                    RouterErrorKind::AuthorizationMetadata,
                    format!("scope template argument `{name}` must be a required supported scalar"),
                ));
            }
        }
    }
    Ok(())
}

fn validate_operation_argument_contract(
    operation: &OperationDescriptor,
    expected: &BTreeMap<String, String>,
) -> Result<(), RouterError> {
    let actual = operation
        .arguments
        .iter()
        .map(|argument| (argument.name.clone(), argument.graphql_type.clone()))
        .collect::<BTreeMap<_, _>>();
    if &actual != expected
        || operation
            .arguments
            .iter()
            .any(|argument| argument.required != argument.graphql_type.ends_with('!'))
    {
        return Err(RouterError::new(
            RouterErrorKind::AuthorizationMetadata,
            format!(
                "authorization argument metadata is stale for {:?}.{}",
                operation.root_type, operation.field_name
            ),
        ));
    }
    Ok(())
}

fn graph_root_contract(graph: &ActiveGraph) -> Result<GraphRootContract, RouterError> {
    let state = &graph.hive.snapshot().planner.supergraph;
    let roots = [
        (RootOperationType::Query, Some(state.query_type.as_str())),
        (RootOperationType::Mutation, state.mutation_type.as_deref()),
        (
            RootOperationType::Subscription,
            state.subscription_type.as_deref(),
        ),
    ];
    let mut fields = BTreeMap::new();
    for (root_type, name) in roots {
        let Some(name) = name else {
            continue;
        };
        let Some(SupergraphDefinition::Object(object)) = state.definitions.get(name) else {
            return Err(RouterError::new(
                RouterErrorKind::AuthorizationMetadata,
                format!("active graph root `{name}` is unavailable to authorization binding"),
            ));
        };
        fields.extend(
            object
                .fields
                .iter()
                .filter(|(name, _)| !name.starts_with('_'))
                .map(|(name, field)| {
                    (
                        (root_type, name.clone()),
                        field
                            .argument_types
                            .iter()
                            .map(|(name, graphql_type)| (name.clone(), graphql_type.to_string()))
                            .collect(),
                    )
                }),
        );
    }
    Ok(fields)
}

fn collect_included_root_fields<'a>(
    selections: &'a [SelectionItem],
    variables: &std::collections::HashMap<String, JsonValue>,
    variable_resolution: VariableResolution,
    fields: &mut Vec<&'a FieldSelection>,
) {
    for selection in selections {
        match selection {
            SelectionItem::Field(field)
                if directive_inclusion(
                    field.skip_if.as_deref(),
                    field.include_if.as_deref(),
                    variables,
                    variable_resolution,
                ) == DirectiveInclusion::Included =>
            {
                fields.push(field);
            }
            SelectionItem::InlineFragment(fragment)
                if directive_inclusion(
                    fragment.skip_if.as_deref(),
                    fragment.include_if.as_deref(),
                    variables,
                    variable_resolution,
                ) == DirectiveInclusion::Included =>
            {
                collect_included_root_fields(
                    &fragment.selections.items,
                    variables,
                    variable_resolution,
                    fields,
                );
            }
            SelectionItem::Field(_)
            | SelectionItem::InlineFragment(_)
            | SelectionItem::FragmentSpread(_) => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirectiveInclusion {
    Included,
    Excluded,
    Deferred,
}

fn directive_inclusion(
    skip_if: Option<&str>,
    include_if: Option<&str>,
    variables: &std::collections::HashMap<String, JsonValue>,
    variable_resolution: VariableResolution,
) -> DirectiveInclusion {
    if variable_resolution == VariableResolution::Preflight
        && [skip_if, include_if]
            .into_iter()
            .flatten()
            .any(|name| !variables.contains_key(name.trim_start_matches('$')))
    {
        return DirectiveInclusion::Deferred;
    }
    let skipped = skip_if
        .and_then(|name| variables.get(name.trim_start_matches('$')))
        .and_then(JsonValueTrait::as_bool)
        .unwrap_or(false);
    let included = include_if
        .and_then(|name| variables.get(name.trim_start_matches('$')))
        .and_then(JsonValueTrait::as_bool)
        .unwrap_or(true);
    if !skipped && included {
        DirectiveInclusion::Included
    } else {
        DirectiveInclusion::Excluded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphql_orm_router_protocol::{
        AdvertisedEndpoint, CapabilitySet, DescriptorFingerprints, Fingerprint, GraphqlEndpoints,
        ProtocolVersion, SchemaAdvertisement, SubgraphId, SubgraphIdentity, SubgraphName,
    };

    const POLICY_SDL: &str = r#"
        extend schema @link(url: "https://specs.apollo.dev/federation/v2.7")
        type Query { item(id: ID!): String, status: String! }
    "#;

    #[test]
    fn principal_and_exact_matcher_reject_malformed_or_non_exact_scopes() {
        assert!(AuthenticatedPrincipal::new("", ["records.read"], None).is_err());
        assert!(AuthenticatedPrincipal::new("subject", ["records read"], None).is_err());
        assert!(ExactScopeMatcher.matches("records.read", "records.read"));
        assert!(!ExactScopeMatcher.matches("records.*", "records.read"));
    }

    #[test]
    fn catalog_rejects_missing_stale_ambiguous_or_incapable_metadata() {
        let graph =
            crate::federation::GraphStore::new(&[crate::federation::CandidateSubgraph::new(
                "catalog",
                "http://catalog.test/graphql",
                POLICY_SDL,
            )])
            .unwrap()
            .load();
        let complete = descriptor(vec![
            OperationDescriptor {
                root_type: RootOperationType::Query,
                field_name: "item".to_owned(),
                arguments: vec![ArgumentDescriptor {
                    name: "id".to_owned(),
                    graphql_type: "ID!".to_owned(),
                    required: true,
                }],
                authorization: AuthorizationRequirement::Authenticated,
            },
            OperationDescriptor {
                root_type: RootOperationType::Query,
                field_name: "status".to_owned(),
                arguments: Vec::new(),
                authorization: AuthorizationRequirement::Public,
            },
        ]);
        assert!(AuthorizationCatalog::build(&graph, std::slice::from_ref(&complete)).is_ok());

        let missing = descriptor(vec![complete.operations[0].clone()]);
        assert_eq!(
            AuthorizationCatalog::build(&graph, &[missing])
                .unwrap_err()
                .kind(),
            RouterErrorKind::AuthorizationMetadata
        );

        let mut stale_operations = complete.operations.clone();
        stale_operations[0].arguments[0].graphql_type = "String!".to_owned();
        let stale = descriptor(stale_operations);
        assert_eq!(
            AuthorizationCatalog::build(&graph, &[stale])
                .unwrap_err()
                .kind(),
            RouterErrorKind::AuthorizationMetadata
        );

        assert_eq!(
            AuthorizationCatalog::build(&graph, &[complete.clone(), complete.clone()])
                .unwrap_err()
                .kind(),
            RouterErrorKind::AuthorizationMetadata
        );

        let mut incapable = complete;
        incapable.capabilities.authorization_metadata = false;
        incapable.fingerprints.combined = incapable.combined_fingerprint();
        assert_eq!(
            AuthorizationCatalog::build(&graph, &[incapable])
                .unwrap_err()
                .kind(),
            RouterErrorKind::AuthorizationMetadata
        );
    }

    fn descriptor(operations: Vec<OperationDescriptor>) -> SubgraphDescriptor {
        let endpoint = |value: &str| AdvertisedEndpoint::try_from(value.to_owned()).unwrap();
        let mut descriptor = SubgraphDescriptor {
            protocol_version: ProtocolVersion { major: 1, minor: 0 },
            subgraph: SubgraphIdentity {
                id: SubgraphId::try_from("catalog-service".to_owned()).unwrap(),
                name: SubgraphName::try_from("catalog".to_owned()).unwrap(),
            },
            graphql: GraphqlEndpoints {
                http: endpoint("http://catalog.test/graphql"),
                websocket: None,
            },
            schema: SchemaAdvertisement {
                url: endpoint("http://catalog.test/sdl"),
            },
            capabilities: CapabilitySet {
                subscriptions: false,
                authorization_metadata: true,
                schema_fingerprints: true,
            },
            required_semantics: Vec::new(),
            operations,
            extensions: Vec::new(),
            fingerprints: DescriptorFingerprints {
                schema: Fingerprint::sha256(POLICY_SDL),
                authorization: Fingerprint::sha256("placeholder"),
                combined: Fingerprint::sha256("placeholder"),
            },
        };
        descriptor.fingerprints.authorization = descriptor.authorization_fingerprint();
        descriptor.fingerprints.combined = descriptor.combined_fingerprint();
        descriptor
    }
}
