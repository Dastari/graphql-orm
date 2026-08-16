//! Reversible provider-specific projections of canonical tool schemas.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::ProviderError;

/// Version of the OpenAI strict-function schema and inverse projection.
pub const OPENAI_STRICT_TOOL_PROJECTION_VERSION: &str = "openai-strict-function-v1";

/// Reviewed reversible projection of one canonical provider-neutral argument
/// schema into the OpenAI strict function contract.
///
/// Canonically optional non-null values become required nullable properties;
/// `null` maps back to omission. Canonically optional values that already
/// admit `null` use a closed `{present,value}` envelope so omission remains
/// distinguishable from an explicitly supplied `null`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiStrictToolProjection {
    canonical_schema: Value,
    provider_schema: Value,
    canonical_schema_fingerprint: String,
    inverse_mapping_fingerprint: String,
    fingerprint: String,
}

impl OpenAiStrictToolProjection {
    /// Compiles and validates one exact reversible strict projection.
    ///
    /// # Errors
    ///
    /// Rejects invalid, unbounded, reference-based, unsupported, lossy, or
    /// non-strict canonical schemas. The canonical value is never mutated.
    pub fn compile(canonical_schema: &Value) -> Result<Self, ProviderError> {
        jsonschema::validator_for(canonical_schema).map_err(|_| {
            ProviderError::InvalidConfiguration(
                "canonical tool argument schema is invalid".to_owned(),
            )
        })?;
        if canonical_schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(invalid_projection());
        }
        let provider_schema = project_schema(canonical_schema)?;
        jsonschema::validator_for(&provider_schema).map_err(|_| invalid_projection())?;
        validate_strict_objects(&provider_schema)?;
        let canonical_schema_fingerprint = fingerprint(canonical_schema);
        let inverse_mapping_fingerprint = fingerprint(&json!({
            "version": OPENAI_STRICT_TOOL_PROJECTION_VERSION,
            "canonical_schema": canonical_schema_fingerprint,
            "inverse": "required-null-to-omission;nullable-optional-present-envelope",
        }));
        let projection_fingerprint = fingerprint(&json!({
            "version": OPENAI_STRICT_TOOL_PROJECTION_VERSION,
            "canonical_schema": canonical_schema,
            "provider_schema": provider_schema,
            "inverse_mapping_fingerprint": inverse_mapping_fingerprint,
        }));
        Ok(Self {
            canonical_schema: canonical_schema.clone(),
            provider_schema,
            canonical_schema_fingerprint,
            inverse_mapping_fingerprint,
            fingerprint: projection_fingerprint,
        })
    }

    /// Exact strict provider schema.
    pub fn provider_schema(&self) -> &Value {
        &self.provider_schema
    }

    /// Fingerprint of the unchanged canonical schema.
    pub fn canonical_schema_fingerprint(&self) -> &str {
        &self.canonical_schema_fingerprint
    }

    /// Fingerprint of the inverse mapping algorithm and canonical schema.
    pub fn inverse_mapping_fingerprint(&self) -> &str {
        &self.inverse_mapping_fingerprint
    }

    /// Complete projection fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Validates provider arguments, reverses the projection, and validates
    /// the resulting unchanged canonical contract.
    ///
    /// # Errors
    ///
    /// Returns a bounded rejection for malformed, ambiguous, or non-canonical
    /// projected arguments.
    pub fn normalize_arguments(&self, arguments: &Value) -> Result<Value, ProviderError> {
        let provider_validator =
            jsonschema::validator_for(&self.provider_schema).map_err(|_| invalid_projection())?;
        if !provider_validator.is_valid(arguments) {
            return Err(ProviderError::Rejected);
        }
        let normalized = normalize_value(&self.canonical_schema, arguments)?;
        let canonical_validator =
            jsonschema::validator_for(&self.canonical_schema).map_err(|_| invalid_projection())?;
        if !canonical_validator.is_valid(&normalized) {
            return Err(ProviderError::Rejected);
        }
        Ok(normalized)
    }
}

fn project_schema(schema: &Value) -> Result<Value, ProviderError> {
    let object = schema.as_object().ok_or_else(invalid_projection)?;
    if object.contains_key("$ref")
        || object.contains_key("patternProperties")
        || object.contains_key("unevaluatedProperties")
        || object.contains_key("dependentSchemas")
        || object.contains_key("propertyNames")
    {
        return Err(invalid_projection());
    }
    if let Some(alternatives) = object.get("anyOf").and_then(Value::as_array) {
        let projected = alternatives
            .iter()
            .map(project_schema)
            .collect::<Result<Vec<_>, _>>()?;
        let mut result = object.clone();
        result.insert("anyOf".to_owned(), Value::Array(projected));
        return Ok(Value::Object(result));
    }
    if let Some(alternatives) = object.get("oneOf").and_then(Value::as_array) {
        let projected = alternatives
            .iter()
            .map(project_schema)
            .collect::<Result<Vec<_>, _>>()?;
        let mut result = object.clone();
        result.insert("oneOf".to_owned(), Value::Array(projected));
        return Ok(Value::Object(result));
    }
    match object.get("type") {
        Some(Value::String(kind)) if kind == "object" => project_object(object),
        Some(Value::String(kind)) if kind == "array" => {
            let items = object.get("items").ok_or_else(invalid_projection)?;
            let mut result = object.clone();
            result.insert("items".to_owned(), project_schema(items)?);
            Ok(Value::Object(result))
        }
        Some(Value::String(_)) | None if object.contains_key("const") => Ok(schema.clone()),
        Some(Value::String(_)) => Ok(schema.clone()),
        Some(Value::Array(_)) => Ok(schema.clone()),
        _ => Err(invalid_projection()),
    }
}

fn project_object(object: &Map<String, Value>) -> Result<Value, ProviderError> {
    if object.get("additionalProperties") != Some(&Value::Bool(false))
        || object.contains_key("minProperties")
        || object.contains_key("maxProperties")
    {
        return Err(invalid_projection());
    }
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(invalid_projection)?;
    let canonical_required = object
        .get("required")
        .and_then(Value::as_array)
        .ok_or_else(invalid_projection)?
        .iter()
        .map(|value| value.as_str().ok_or_else(invalid_projection))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if canonical_required
        .iter()
        .any(|name| !properties.contains_key(*name))
    {
        return Err(invalid_projection());
    }
    let mut projected_properties = Map::new();
    for (name, property) in properties {
        let projected = project_schema(property)?;
        let projected = if canonical_required.contains(name.as_str()) {
            projected
        } else if schema_allows_null(property)? {
            json!({
                "oneOf": [
                    {
                        "type": "object",
                        "properties": {
                            "present": { "const": false },
                            "value": { "type": "null" },
                        },
                        "required": ["present", "value"],
                        "additionalProperties": false,
                    },
                    {
                        "type": "object",
                        "properties": {
                            "present": { "const": true },
                            "value": projected,
                        },
                        "required": ["present", "value"],
                        "additionalProperties": false,
                    }
                ]
            })
        } else {
            json!({ "anyOf": [projected, { "type": "null" }] })
        };
        projected_properties.insert(name.clone(), projected);
    }
    let mut result = object.clone();
    result.insert("properties".to_owned(), Value::Object(projected_properties));
    result.insert(
        "required".to_owned(),
        Value::Array(properties.keys().cloned().map(Value::String).collect()),
    );
    result.insert("additionalProperties".to_owned(), Value::Bool(false));
    Ok(Value::Object(result))
}

fn normalize_value(schema: &Value, value: &Value) -> Result<Value, ProviderError> {
    let object = schema.as_object().ok_or(ProviderError::Rejected)?;
    if let Some(alternatives) = object
        .get("anyOf")
        .or_else(|| object.get("oneOf"))
        .and_then(Value::as_array)
    {
        let mut candidates = Vec::new();
        for alternative in alternatives {
            let projected = project_schema(alternative).map_err(|_| ProviderError::Rejected)?;
            let validator =
                jsonschema::validator_for(&projected).map_err(|_| ProviderError::Rejected)?;
            if validator.is_valid(value)
                && let Ok(candidate) = normalize_value(alternative, value)
            {
                let canonical =
                    jsonschema::validator_for(alternative).map_err(|_| ProviderError::Rejected)?;
                if canonical.is_valid(&candidate) && !candidates.contains(&candidate) {
                    candidates.push(candidate);
                }
            }
        }
        return match candidates.as_slice() {
            [candidate] => Ok(candidate.clone()),
            _ => Err(ProviderError::Rejected),
        };
    }
    match object.get("type") {
        Some(Value::String(kind)) if kind == "object" => normalize_object(object, value),
        Some(Value::String(kind)) if kind == "array" => {
            let items = object.get("items").ok_or(ProviderError::Rejected)?;
            let values = value.as_array().ok_or(ProviderError::Rejected)?;
            Ok(Value::Array(
                values
                    .iter()
                    .map(|value| normalize_value(items, value))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        _ => Ok(value.clone()),
    }
}

fn normalize_object(schema: &Map<String, Value>, value: &Value) -> Result<Value, ProviderError> {
    let values = value.as_object().ok_or(ProviderError::Rejected)?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(ProviderError::Rejected)?;
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .ok_or(ProviderError::Rejected)?
        .iter()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let mut normalized = Map::new();
    for (name, property) in properties {
        let projected_value = values.get(name).ok_or(ProviderError::Rejected)?;
        if required.contains(name.as_str()) {
            normalized.insert(name.clone(), normalize_value(property, projected_value)?);
        } else if schema_allows_null(property).map_err(|_| ProviderError::Rejected)? {
            let envelope = projected_value.as_object().ok_or(ProviderError::Rejected)?;
            match envelope.get("present").and_then(Value::as_bool) {
                Some(false) if envelope.get("value") == Some(&Value::Null) => {}
                Some(true) => {
                    normalized.insert(
                        name.clone(),
                        normalize_value(
                            property,
                            envelope.get("value").ok_or(ProviderError::Rejected)?,
                        )?,
                    );
                }
                _ => return Err(ProviderError::Rejected),
            }
        } else if !projected_value.is_null() {
            normalized.insert(name.clone(), normalize_value(property, projected_value)?);
        }
    }
    Ok(Value::Object(normalized))
}

fn schema_allows_null(schema: &Value) -> Result<bool, ProviderError> {
    let validator = jsonschema::validator_for(schema).map_err(|_| invalid_projection())?;
    Ok(validator.is_valid(&Value::Null))
}

fn validate_strict_objects(schema: &Value) -> Result<(), ProviderError> {
    let object = schema.as_object().ok_or_else(invalid_projection)?;
    if object.get("type").and_then(Value::as_str) == Some("object") {
        let properties = object
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(invalid_projection)?;
        let required = object
            .get("required")
            .and_then(Value::as_array)
            .ok_or_else(invalid_projection)?
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        if object.get("additionalProperties") != Some(&Value::Bool(false))
            || properties
                .keys()
                .any(|name| !required.contains(name.as_str()))
            || required.len() != properties.len()
        {
            return Err(invalid_projection());
        }
        for property in properties.values() {
            validate_strict_objects(property)?;
        }
    }
    if let Some(items) = object.get("items") {
        validate_strict_objects(items)?;
    }
    for alternatives in [object.get("anyOf"), object.get("oneOf")]
        .into_iter()
        .flatten()
    {
        for alternative in alternatives.as_array().ok_or_else(invalid_projection)? {
            validate_strict_objects(alternative)?;
        }
    }
    Ok(())
}

fn fingerprint(value: &Value) -> String {
    let encoded = serde_json::to_vec(value).unwrap_or_default();
    hex::encode(Sha256::digest(encoded))
}

fn invalid_projection() -> ProviderError {
    ProviderError::InvalidConfiguration(
        "tool schema has no reversible OpenAI strict projection".to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_projection_round_trips_optional_and_nullable_values() {
        let canonical = json!({
            "type": "object",
            "properties": {
                "required": { "type": "string" },
                "optional": { "type": "integer" },
                "nullableOptional": { "anyOf": [{"type": "string"}, {"type": "null"}] },
                "nested": {
                    "type": "object",
                    "properties": { "value": { "type": "string" } },
                    "required": [],
                    "additionalProperties": false
                }
            },
            "required": ["required", "nested"],
            "additionalProperties": false
        });
        let projection = OpenAiStrictToolProjection::compile(&canonical).expect("projection");
        validate_strict_objects(projection.provider_schema()).expect("strict");
        let normalized = projection
            .normalize_arguments(&json!({
                "required": "value",
                "optional": null,
                "nullableOptional": {"present": true, "value": null},
                "nested": {"value": null}
            }))
            .expect("normalize");
        assert_eq!(
            normalized,
            json!({"required": "value", "nullableOptional": null, "nested": {}})
        );
    }

    #[test]
    fn strict_projection_rejects_open_additional_properties() {
        let schema = json!({
            "type": "object",
            "properties": {},
            "required": []
        });
        assert!(OpenAiStrictToolProjection::compile(&schema).is_err());
    }
}
