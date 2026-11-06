//! Exact private-query admission before request context construction.
#![deny(missing_docs)]
use super::AgqlAcceptedRequest;
use agql_auth::{
    AccessTokenGrantKind, AuthPrincipal, AuthResult, AuthUser, Clock,
    VerifiedActiveUserSessionResolver,
};
use async_graphql::parser::{
    parse_query,
    types::{BaseType, DocumentOperations, OperationType, Selection, SelectionSet, Type},
};
use async_graphql_value::Value as GqlValue;
use graphql_orm_operation_catalog::{
    RECEIVER_PURPOSE_CLAIM, ReceiverPurposeMetadata, ReceiverPurposeV1, exact_document_hash,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use subtle::ConstantTimeEq;

/// Uniform public refusal; does not disclose the failing check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgqlReceiverRefused;
impl std::fmt::Display for AgqlReceiverRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RECEIVER_ADMISSION_REFUSED")
    }
}
impl std::error::Error for AgqlReceiverRefused {}
type Result<T> = std::result::Result<T, AgqlReceiverRefused>;
fn require(condition: bool) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(AgqlReceiverRefused)
    }
}
fn equal(a: &str, b: &str) -> bool {
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// Explicit transport boundary. Version 1 admits HTTP only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgqlReceiverTransport {
    /// One bounded raw HTTP body.
    Http,
    /// Always refused, including connection-init and per-event requests.
    WebSocket,
}

/// Host current scope/tenant policy. No default implementation is supplied.
/// Implementations must perform authoritative reads and must not dispatch
/// protected work. Parent liveness does not establish MFA or assurance.
#[async_trait::async_trait]
pub trait AgqlReceiverPolicy: Send + Sync {
    /// Applies current scope, tenant, organization and resource policy.
    /// Check any tenant/resource selectors in the raw variables and derived
    /// root arguments against current authority; signed intent alone is not policy.
    /// # Errors
    /// Errors and false both refuse admission.
    async fn permits(
        &self,
        delegated: &AuthUser,
        parent: &AuthUser,
        contract: &ReceiverPurposeMetadata,
        variables: &Value,
        arguments: &Value,
    ) -> AuthResult<bool>;
}

/// Immutable exact query contract. Registration accepts only builtin scalar
/// variable types (and lists), direct variable root arguments, one named query/root,
/// and plain field selections.
/// Hosts must register the document against their actual schema and ensure
/// variable types match root argument types; custom input types are refused.
#[derive(Clone)]
pub struct AgqlReceiverContract {
    name: String,
    document: String,
    root: String,
    metadata: ReceiverPurposeMetadata,
}
impl AgqlReceiverContract {
    /// Validates a server-authored contract. No attacker input is normalized.
    /// # Errors
    /// Refuses unsupported documents or absent metadata.
    pub fn new(
        name: &str,
        document: &str,
        root: &str,
        metadata: ReceiverPurposeMetadata,
    ) -> Result<Self> {
        require(!name.is_empty() && !root.is_empty() && document.len() <= 65536)?;
        require(
            [
                &metadata.receiver,
                &metadata.target,
                &metadata.schema,
                &metadata.capability,
                &metadata.projection,
                &metadata.disclosure,
                &metadata.resource_type,
                &metadata.resource_id,
            ]
            .iter()
            .all(|s| !s.trim().is_empty()),
        )?;
        let contract = Self {
            name: name.into(),
            document: document.into(),
            root: root.into(),
            metadata,
        };
        contract.arguments(&serde_json::json!({}), false)?;
        Ok(contract)
    }
    fn arguments(&self, variables: &Value, check_values: bool) -> Result<Value> {
        let parsed = parse_query(&self.document).map_err(|_| AgqlReceiverRefused)?;
        require(parsed.fragments.is_empty())?;
        let DocumentOperations::Multiple(operations) = parsed.operations else {
            return Err(AgqlReceiverRefused);
        };
        require(operations.len() == 1)?;
        let operation = &operations
            .get(self.name.as_str())
            .ok_or(AgqlReceiverRefused)?
            .node;
        require(
            operation.ty == OperationType::Query
                && operation.directives.is_empty()
                && operation.selection_set.node.items.len() == 1,
        )?;
        plain_selection(&operation.selection_set.node)?;
        let Selection::Field(root) = &operation.selection_set.node.items[0].node else {
            return Err(AgqlReceiverRefused);
        };
        require(root.node.name.node.as_str() == self.root)?;
        let raw = variables.as_object().ok_or(AgqlReceiverRefused)?;
        let mut declared = BTreeSet::new();
        let mut coerced = BTreeMap::new();
        for definition in &operation.variable_definitions {
            let definition = &definition.node;
            let name = definition.name.node.to_string();
            require(declared.insert(name.clone()) && definition.directives.is_empty())?;
            supported(&definition.var_type.node)?;
            let default = definition
                .default_value
                .as_ref()
                .map(|v| serde_json::to_value(&v.node).map_err(|_| AgqlReceiverRefused))
                .transpose()?;
            if let Some(default) = &default {
                coerce(default, &definition.var_type.node)?;
            }
            if check_values {
                if let Some(value) = raw.get(&name).or(default.as_ref()) {
                    coerced.insert(name, coerce(value, &definition.var_type.node)?);
                } else {
                    require(definition.var_type.node.nullable)?;
                }
            }
        }
        require(raw.keys().all(|key| declared.contains(key)))?;
        let mut arguments = serde_json::Map::new();
        let mut used = BTreeSet::new();
        for (name, value) in &root.node.arguments {
            require(used.insert(name.node.to_string()))?;
            // Literal argument coercion depends on schema types, which this
            // boundary does not own. Admit only direct typed variables.
            let value = match &value.node {
                GqlValue::Variable(variable) => {
                    require(declared.contains(variable.as_str()))?;
                    coerced.get(variable.as_str()).cloned()
                }
                _ => return Err(AgqlReceiverRefused),
            };
            if let Some(value) = value {
                arguments.insert(name.node.to_string(), value);
            }
        }
        Ok(Value::Object(arguments))
    }
}
fn plain_selection(selection: &SelectionSet) -> Result<()> {
    let mut names = BTreeSet::new();
    for item in &selection.items {
        let Selection::Field(field) = &item.node else {
            return Err(AgqlReceiverRefused);
        };
        require(
            field.node.alias.is_none()
                && field.node.directives.is_empty()
                && names.insert(field.node.name.node.to_string()),
        )?;
        // Nested arguments need schema-specific coercion and are outside v1.
        if !field.node.selection_set.node.items.is_empty() {
            for child in &field.node.selection_set.node.items {
                if let Selection::Field(child) = &child.node {
                    require(child.node.arguments.is_empty())?;
                }
            }
            plain_selection(&field.node.selection_set.node)?;
        }
    }
    Ok(())
}
fn supported(ty: &Type) -> Result<()> {
    match &ty.base {
        BaseType::List(inner) => supported(inner),
        BaseType::Named(name) => require(matches!(
            name.as_str(),
            "String" | "ID" | "Boolean" | "Int" | "Float"
        )),
    }
}
fn coerce(value: &Value, ty: &Type) -> Result<Value> {
    if value.is_null() {
        require(ty.nullable)?;
        return Ok(Value::Null);
    }
    match &ty.base {
        BaseType::List(inner) => {
            let values = match value {
                Value::Array(values) => values.clone(),
                value => vec![value.clone()],
            };
            Ok(Value::Array(
                values
                    .iter()
                    .map(|v| coerce(v, inner))
                    .collect::<Result<_>>()?,
            ))
        }
        BaseType::Named(name) => {
            let value = match name.as_str() {
                "String" if value.is_string() => value.clone(),
                "ID" if value.is_string() => value.clone(),
                "ID" if value.as_i64().is_some() || value.as_u64().is_some() => {
                    Value::String(value.to_string())
                }
                "Boolean" if value.is_boolean() => value.clone(),
                "Int" if value.as_i64().is_some_and(|n| i32::try_from(n).is_ok()) => value.clone(),
                "Float" if value.is_number() => {
                    serde_json::json!(value.as_f64().ok_or(AgqlReceiverRefused)?)
                }
                _ => return Err(AgqlReceiverRefused),
            };
            Ok(value)
        }
    }
}

/// Immutable default-deny registry, local to one receiver/schema deployment.
pub struct AgqlReceiverRegistry {
    contracts: BTreeMap<String, AgqlReceiverContract>,
    max_body_bytes: usize,
}
impl AgqlReceiverRegistry {
    /// Creates a registry; duplicate names and invalid body bounds are refused.
    /// # Errors
    /// Refuses duplicate contracts or a body bound outside 1..=1 MiB.
    pub fn new(contracts: Vec<AgqlReceiverContract>, max_body_bytes: usize) -> Result<Self> {
        require((1..=1048576).contains(&max_body_bytes))?;
        let mut registered = BTreeMap::new();
        for contract in contracts {
            require(registered.insert(contract.name.clone(), contract).is_none())?;
        }
        Ok(Self {
            contracts: registered,
            max_body_bytes,
        })
    }
    /// Admits raw HTTP JSON before constructing async-graphql requests or exposing scopes.
    /// The host must already authenticate signature, issuer, audience, token status,
    /// and any sender proof. It must also bound reads while buffering the body.
    /// This check does not replace resolver/row/field authorization or fence later revocation.
    /// # Errors
    /// Every absent, malformed, unsupported or unauthorized element uses the same refusal.
    pub async fn admit(
        &self,
        body: &[u8],
        transport: AgqlReceiverTransport,
        principal: AuthPrincipal,
        resolver: &dyn VerifiedActiveUserSessionResolver,
        policy: &dyn AgqlReceiverPolicy,
        clock: &dyn Clock,
    ) -> Result<AgqlAdmittedReceiverRequest> {
        require(transport == AgqlReceiverTransport::Http && body.len() <= self.max_body_bytes)?;
        let strict: StrictJson = serde_json::from_slice(body).map_err(|_| AgqlReceiverRefused)?;
        let raw = strict.0.as_object().ok_or(AgqlReceiverRefused)?;
        require(
            raw.keys()
                .all(|k| matches!(k.as_str(), "query" | "operationName" | "variables")),
        )?;
        let name = raw
            .get("operationName")
            .and_then(Value::as_str)
            .ok_or(AgqlReceiverRefused)?;
        let document = raw
            .get("query")
            .and_then(Value::as_str)
            .ok_or(AgqlReceiverRefused)?;
        let contract = self.contracts.get(name).ok_or(AgqlReceiverRefused)?;
        // Exact bytes first. Only the registered document is subsequently parsed.
        require(document.as_bytes() == contract.document.as_bytes())?;
        let variables = raw
            .get("variables")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let arguments = contract.arguments(&variables, true)?;
        let AuthPrincipal::User(user) = &principal else {
            return Err(AgqlReceiverRefused);
        };
        let claims = &user.token_claims;
        require(
            claims.grant_kind == Some(AccessTokenGrantKind::SessionBoundDelegation)
                && claims.authorization_roles.is_empty(),
        )?;
        require(
            claims.actor.as_ref().is_some_and(|a| !a.sub.is_empty())
                && claims
                    .correlation_id
                    .as_ref()
                    .is_some_and(|v| !v.is_empty()),
        )?;
        require(
            claims
                .session_family_id
                .as_ref()
                .is_some_and(|v| !v.is_empty())
                && claims
                    .session_version
                    .as_ref()
                    .is_some_and(|v| !v.is_empty()),
        )?;
        require(
            claims.tenant_id.as_ref().is_some_and(|v| !v.is_empty())
                && claims
                    .organization_id
                    .as_ref()
                    .is_some_and(|v| !v.is_empty()),
        )?;
        let expiry = claims.expires_at.ok_or(AgqlReceiverRefused)?;
        require(expiry > clock.now())?;
        let operation = claims.operation.as_ref().ok_or(AgqlReceiverRefused)?;
        require(
            operation.operation_name == name
                && equal(&operation.document_sha256, &exact_document_hash(document)),
        )?;
        require(
            claims.resource_type.as_ref() == Some(&contract.metadata.resource_type)
                && claims.resource_id.as_ref() == Some(&contract.metadata.resource_id),
        )?;
        let purpose: ReceiverPurposeV1 = serde_json::from_value(
            claims
                .additional
                .get(RECEIVER_PURPOSE_CLAIM)
                .cloned()
                .ok_or(AgqlReceiverRefused)?,
        )
        .map_err(|_| AgqlReceiverRefused)?;
        let expected = ReceiverPurposeV1::bind(
            contract.metadata.clone(),
            name,
            document,
            &variables,
            &arguments,
        );
        require(
            purpose.version == 1
                && purpose.target_class == "private_direct"
                && purpose.metadata == expected.metadata
                && purpose.operation_name == expected.operation_name
                && equal(&purpose.document_sha256, &expected.document_sha256)
                && equal(&purpose.variables_sha256, &expected.variables_sha256)
                && equal(&purpose.arguments_sha256, &expected.arguments_sha256),
        )?;
        let reference = principal.reference();
        let parent = resolver
            .resolve_active_user_session(&reference)
            .await
            .map_err(|_| AgqlReceiverRefused)?;
        let current = parent.user();
        require(
            parent.reference() == &reference
                && claims.session_version.as_deref() == Some(parent.session_version())
                && parent.verified_at() <= clock.now()
                && clock.now().unix_timestamp_nanos() - parent.verified_at().unix_timestamp_nanos()
                    <= 30_000_000_000,
        )?;
        require(
            current.token_claims.grant_kind == Some(AccessTokenGrantKind::UserSession)
                && current.token_claims.organization_id == claims.organization_id
                && current.token_claims.tenant_id == claims.tenant_id,
        )?;
        require(
            user.roles.iter().all(|v| current.roles.contains(v))
                && user.scopes.iter().all(|v| current.scopes.contains(v)),
        )?;
        require(
            policy
                .permits(user, current, &contract.metadata, &variables, &arguments)
                .await
                .map_err(|_| AgqlReceiverRefused)?,
        )?;
        // Re-read after the host policy await. Never infer assurance from
        // parent liveness or compare MFA/assurance snapshots here.
        let latest = resolver
            .resolve_active_user_session(&reference)
            .await
            .map_err(|_| AgqlReceiverRefused)?;
        let latest_user = latest.user();
        require(
            latest.reference() == &reference
                && claims.session_version.as_deref() == Some(latest.session_version())
                && latest.verified_at() <= clock.now()
                && clock.now().unix_timestamp_nanos() - latest.verified_at().unix_timestamp_nanos()
                    <= 30_000_000_000
                && latest_user.token_claims.grant_kind == Some(AccessTokenGrantKind::UserSession)
                && latest_user.token_claims.organization_id == claims.organization_id
                && latest_user.token_claims.tenant_id == claims.tenant_id
                && latest_user.roles == current.roles
                && latest_user.scopes == current.scopes,
        )?;
        require(
            expiry > clock.now()
                && parent.absolute_expires_at() > clock.now()
                && parent.idle_expires_at().is_none_or(|e| e > clock.now())
                && latest.absolute_expires_at() > clock.now()
                && latest.idle_expires_at().is_none_or(|e| e > clock.now()),
        )?;
        Ok(AgqlAdmittedReceiverRequest {
            accepted: AgqlAcceptedRequest::new(principal),
            document: document.into(),
            name: name.into(),
            variables,
        })
    }
}

/// Sealed successful admission. Cannot be constructed or deserialized by hosts.
/// Authority is available only after all receiver checks complete. Use promptly;
/// long-running operations must separately revalidate current authority.
#[must_use = "Dispatch this admitted request before exposing delegated authority"]
pub struct AgqlAdmittedReceiverRequest {
    accepted: AgqlAcceptedRequest,
    document: String,
    name: String,
    variables: Value,
}
impl AgqlAdmittedReceiverRequest {
    /// Returns the unchanged delegated scopes. No roles/scopes are expanded.
    pub fn scopes(&self) -> &[String] {
        self.accepted.subject().scopes.as_slice()
    }
    /// Constructs the admitted request and injects coherent accepted identity.
    /// # Errors
    /// Returns a safe GraphQL error if identity cannot be installed.
    pub fn into_request(self) -> async_graphql::Result<async_graphql::Request> {
        let request = async_graphql::Request::new(self.document)
            .operation_name(self.name)
            .variables(async_graphql::Variables::from_json(self.variables));
        self.accepted.inject(request)
    }
}

// Recursive duplicate rejection before serde_json can discard duplicate keys.
struct StrictJson(Value);
impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StrictJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("unique-key JSON")
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                v: bool,
            ) -> std::result::Result<StrictJson, E> {
                Ok(StrictJson(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<StrictJson, E> {
                Ok(StrictJson(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<StrictJson, E> {
                Ok(StrictJson(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<StrictJson, E> {
                serde_json::Number::from_f64(v)
                    .map(|v| StrictJson(Value::Number(v)))
                    .ok_or_else(|| E::custom("invalid JSON"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<StrictJson, E> {
                Ok(StrictJson(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<StrictJson, E> {
                Ok(StrictJson(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<StrictJson, A::Error> {
                let mut values = Vec::new();
                while let Some(StrictJson(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(StrictJson(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<StrictJson, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, StrictJson(value))) = map.next_entry::<String, StrictJson>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                }
                Ok(StrictJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
