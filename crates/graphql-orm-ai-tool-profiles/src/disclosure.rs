//! Static, server-owned disclosure schemas for model-visible tool results.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{AiError, DataClassification};

const MAXIMUM_DISCLOSURE_SCHEMA_DEPTH: usize = 64;

/// Whether a selected result node may ever leave the application boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiDisclosureDisposition {
    /// The node may be disclosed subject to its classification and egress policy.
    Exportable,
    /// The node is structurally forbidden from model/provider disclosure.
    NeverExport,
}

/// Static disclosure policy shared by a result node and its descendants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiDisclosureRule {
    /// Minimum confidentiality classification assigned by server-owned metadata.
    pub classification: DataClassification,
    /// Structural export eligibility.
    pub disposition: AiDisclosureDisposition,
}

impl AiDisclosureRule {
    /// Creates an exportable rule with the supplied minimum classification.
    pub const fn exportable(classification: DataClassification) -> Self {
        Self {
            classification,
            disposition: AiDisclosureDisposition::Exportable,
        }
    }

    /// Creates a node that must never be included in model-facing output.
    pub const fn never_export(classification: DataClassification) -> Self {
        Self {
            classification,
            disposition: AiDisclosureDisposition::NeverExport,
        }
    }
}

/// Exact recursive shape of a server-owned result projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum AiDisclosureShape {
    /// A JSON string, number, boolean, or null value.
    Scalar {
        /// Static rule for this value.
        rule: AiDisclosureRule,
    },
    /// An object with a closed set of allowed fields.
    Object {
        /// Static rule for the object container.
        rule: AiDisclosureRule,
        /// Server-owned field shapes. Unknown result fields are rejected.
        fields: BTreeMap<String, AiDisclosureShape>,
    },
    /// A bounded list whose items all use one static shape.
    List {
        /// Static rule for the list container.
        rule: AiDisclosureRule,
        /// Maximum number of model-visible list entries.
        maximum_items: u32,
        /// Static item shape.
        item: Box<AiDisclosureShape>,
    },
}

impl AiDisclosureShape {
    /// Creates a scalar shape.
    pub const fn scalar(rule: AiDisclosureRule) -> Self {
        Self::Scalar { rule }
    }

    /// Creates a closed object shape.
    pub fn object(
        rule: AiDisclosureRule,
        fields: impl IntoIterator<Item = (String, AiDisclosureShape)>,
    ) -> Self {
        Self::Object {
            rule,
            fields: fields.into_iter().collect(),
        }
    }

    /// Creates a bounded list shape.
    pub fn list(rule: AiDisclosureRule, maximum_items: u32, item: AiDisclosureShape) -> Self {
        Self::List {
            rule,
            maximum_items,
            item: Box::new(item),
        }
    }

    fn validate(&self, depth: usize) -> Result<(), AiError> {
        if depth > MAXIMUM_DISCLOSURE_SCHEMA_DEPTH {
            return Err(AiError::InvalidConfiguration(
                "disclosure schema exceeds maximum nesting depth".to_owned(),
            ));
        }
        match self {
            Self::Scalar { .. } => Ok(()),
            Self::Object { fields, .. } => {
                if fields
                    .keys()
                    .any(|field| field.is_empty() || field.starts_with("__"))
                {
                    return Err(AiError::InvalidConfiguration(
                        "disclosure schema contains an invalid field".to_owned(),
                    ));
                }
                for shape in fields.values() {
                    shape.validate(depth + 1)?;
                }
                Ok(())
            }
            Self::List {
                maximum_items,
                item,
                ..
            } => {
                if *maximum_items == 0 {
                    return Err(AiError::InvalidConfiguration(
                        "disclosure list limit must be positive".to_owned(),
                    ));
                }
                item.validate(depth + 1)
            }
        }
    }
}

/// Fingerprint-bound disclosure contract for one exact tool projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiDisclosureSchema {
    /// Host-controlled immutable schema version.
    pub version: String,
    /// Exact recursive projection shape.
    pub root: AiDisclosureShape,
    /// Stable fingerprint over the complete versioned schema.
    pub fingerprint: String,
}

impl AiDisclosureSchema {
    /// Creates and fingerprints a validated static disclosure schema.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for an empty version, invalid
    /// field name, zero list bound, or excessive schema nesting.
    pub fn new(version: impl Into<String>, root: AiDisclosureShape) -> Result<Self, AiError> {
        let version = version.into();
        if version.trim().is_empty() {
            return Err(AiError::InvalidConfiguration(
                "disclosure schema version must not be empty".to_owned(),
            ));
        }
        root.validate(0)?;
        let mut schema = Self {
            version,
            root,
            fingerprint: String::new(),
        };
        schema.refresh_fingerprint();
        Ok(schema)
    }

    /// Evaluates an exact JSON result against the static shape.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed error for unknown fields, shape mismatches,
    /// oversized lists, or any selected `NeverExport` node.
    pub fn evaluate(
        &self,
        value: &serde_json::Value,
    ) -> Result<AiDisclosureEvaluation, AiDisclosureError> {
        evaluate_node(value, &self.root, 0)
    }

    /// Returns the checked worst-case record count for an application GraphQL
    /// result envelope.
    ///
    /// The disclosure root and its one selected GraphQL root field are
    /// transport envelopes and do not count. Each object or scalar result row
    /// below that field counts once. List entries count as records, sibling
    /// collections add, and nested collection fanout multiplies. `None`
    /// reports arithmetic overflow or an ambiguous envelope shape.
    pub fn maximum_graphql_record_count(&self) -> Option<u64> {
        let AiDisclosureShape::Object { fields, .. } = &self.root else {
            return None;
        };
        if fields.len() != 1 {
            return None;
        }
        let result = fields.values().next()?;
        maximum_record_count(result)
    }

    /// Evaluates a GraphQL result and enforces an exact total record ceiling.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed error for the ordinary disclosure failures, an
    /// ambiguous GraphQL envelope, arithmetic overflow, or a total record
    /// count above `maximum_records`.
    pub fn evaluate_graphql_with_record_limit(
        &self,
        value: &serde_json::Value,
        maximum_records: u32,
    ) -> Result<AiDisclosureEvaluation, AiDisclosureError> {
        if maximum_records == 0 {
            return Err(AiDisclosureError::RecordLimitExceeded);
        }
        let evaluation = self.evaluate(value)?;
        let object = value.as_object().ok_or(AiDisclosureError::ShapeMismatch)?;
        let AiDisclosureShape::Object { fields, .. } = &self.root else {
            return Err(AiDisclosureError::ShapeMismatch);
        };
        if fields.len() != 1 {
            return Err(AiDisclosureError::ShapeMismatch);
        }
        let (field_name, result_shape) = fields
            .iter()
            .next()
            .ok_or(AiDisclosureError::ShapeMismatch)?;
        let result = object
            .get(field_name)
            .ok_or(AiDisclosureError::ShapeMismatch)?;
        let actual = actual_record_count(result, result_shape, 0)?;
        if actual > u64::from(maximum_records) {
            return Err(AiDisclosureError::RecordLimitExceeded);
        }
        Ok(evaluation)
    }

    /// Returns the greatest explicit list-cardinality bound in the schema.
    #[doc(hidden)]
    pub fn maximum_list_bound(&self) -> u32 {
        maximum_list_bound(&self.root)
    }

    fn refresh_fingerprint(&mut self) {
        self.fingerprint.clear();
        let encoded = serde_json::to_vec(self)
            .expect("AiDisclosureSchema consists only of serializable values");
        self.fingerprint = hex::encode(Sha256::digest(encoded));
    }
}

fn maximum_list_bound(shape: &AiDisclosureShape) -> u32 {
    match shape {
        AiDisclosureShape::Scalar { .. } => 0,
        AiDisclosureShape::Object { fields, .. } => {
            fields.values().map(maximum_list_bound).max().unwrap_or(0)
        }
        AiDisclosureShape::List {
            maximum_items,
            item,
            ..
        } => (*maximum_items).max(maximum_list_bound(item)),
    }
}

fn maximum_record_count(shape: &AiDisclosureShape) -> Option<u64> {
    match shape {
        AiDisclosureShape::Scalar { .. } => Some(1),
        AiDisclosureShape::Object { fields, .. } => {
            fields.values().try_fold(1_u64, |total, field| {
                total.checked_add(maximum_nested_record_count(field)?)
            })
        }
        AiDisclosureShape::List {
            maximum_items,
            item,
            ..
        } => u64::from(*maximum_items).checked_mul(maximum_record_count(item)?),
    }
}

fn maximum_nested_record_count(shape: &AiDisclosureShape) -> Option<u64> {
    match shape {
        AiDisclosureShape::Scalar { .. } => Some(0),
        AiDisclosureShape::Object { .. } | AiDisclosureShape::List { .. } => {
            maximum_record_count(shape)
        }
    }
}

/// Safe summary produced after a result conforms to its static disclosure schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiDisclosureEvaluation {
    /// Highest effective static classification in the selected result.
    pub maximum_classification: DataClassification,
    /// Number of selected JSON nodes checked against server-owned metadata.
    pub selected_node_count: u64,
}

impl AiDisclosureEvaluation {
    /// Applies a runtime classification that may only tighten the static result.
    pub fn tighten(self, runtime_minimum: DataClassification) -> Self {
        Self {
            maximum_classification: self.maximum_classification.max(runtime_minimum),
            selected_node_count: self.selected_node_count,
        }
    }
}

/// Fail-closed disclosure validation error with no result content.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiDisclosureError {
    /// A selected node is structurally forbidden from model/provider disclosure.
    #[error("tool result contains a non-exportable field")]
    NeverExport,
    /// A result object contains a field absent from server-owned metadata.
    #[error("tool result contains an unknown field")]
    UnknownField,
    /// A result node does not match the registered projection shape.
    #[error("tool result does not match its disclosure schema")]
    ShapeMismatch,
    /// A result list exceeds its registered model-visible item bound.
    #[error("tool result exceeds its disclosure list bound")]
    ListLimitExceeded,
    /// The complete result contains more records than its exact descriptor
    /// budget, including sibling and nested collections.
    #[error("tool result exceeds its total record bound")]
    RecordLimitExceeded,
}

fn evaluate_node(
    value: &serde_json::Value,
    shape: &AiDisclosureShape,
    depth: usize,
) -> Result<AiDisclosureEvaluation, AiDisclosureError> {
    if depth > MAXIMUM_DISCLOSURE_SCHEMA_DEPTH {
        return Err(AiDisclosureError::ShapeMismatch);
    }

    let rule = match shape {
        AiDisclosureShape::Scalar { rule }
        | AiDisclosureShape::Object { rule, .. }
        | AiDisclosureShape::List { rule, .. } => *rule,
    };
    if rule.disposition == AiDisclosureDisposition::NeverExport {
        return Err(AiDisclosureError::NeverExport);
    }

    let mut evaluation = AiDisclosureEvaluation {
        maximum_classification: rule.classification,
        selected_node_count: 1,
    };
    if value.is_null() {
        return Ok(evaluation);
    }

    match shape {
        AiDisclosureShape::Scalar { .. } => {
            if value.is_string() || value.is_number() || value.is_boolean() {
                Ok(evaluation)
            } else {
                Err(AiDisclosureError::ShapeMismatch)
            }
        }
        AiDisclosureShape::Object { fields, .. } => {
            let object = value.as_object().ok_or(AiDisclosureError::ShapeMismatch)?;
            for (field, field_value) in object {
                let field_shape = fields.get(field).ok_or(AiDisclosureError::UnknownField)?;
                merge_evaluation(
                    &mut evaluation,
                    evaluate_node(field_value, field_shape, depth + 1)?,
                );
            }
            Ok(evaluation)
        }
        AiDisclosureShape::List {
            maximum_items,
            item,
            ..
        } => {
            let list = value.as_array().ok_or(AiDisclosureError::ShapeMismatch)?;
            if list.len() > *maximum_items as usize {
                return Err(AiDisclosureError::ListLimitExceeded);
            }
            for item_value in list {
                merge_evaluation(&mut evaluation, evaluate_node(item_value, item, depth + 1)?);
            }
            Ok(evaluation)
        }
    }
}

fn merge_evaluation(target: &mut AiDisclosureEvaluation, source: AiDisclosureEvaluation) {
    target.maximum_classification = target
        .maximum_classification
        .max(source.maximum_classification);
    target.selected_node_count = target
        .selected_node_count
        .saturating_add(source.selected_node_count);
}

fn actual_record_count(
    value: &serde_json::Value,
    shape: &AiDisclosureShape,
    depth: usize,
) -> Result<u64, AiDisclosureError> {
    if depth > MAXIMUM_DISCLOSURE_SCHEMA_DEPTH {
        return Err(AiDisclosureError::ShapeMismatch);
    }
    if value.is_null() {
        return Ok(0);
    }
    match shape {
        AiDisclosureShape::Scalar { .. } => Ok(1),
        AiDisclosureShape::Object { fields, .. } => {
            let object = value.as_object().ok_or(AiDisclosureError::ShapeMismatch)?;
            object.iter().try_fold(1_u64, |total, (name, value)| {
                let shape = fields.get(name).ok_or(AiDisclosureError::UnknownField)?;
                total
                    .checked_add(actual_nested_record_count(value, shape, depth + 1)?)
                    .ok_or(AiDisclosureError::RecordLimitExceeded)
            })
        }
        AiDisclosureShape::List {
            maximum_items,
            item,
            ..
        } => {
            let list = value.as_array().ok_or(AiDisclosureError::ShapeMismatch)?;
            if list.len() > *maximum_items as usize {
                return Err(AiDisclosureError::ListLimitExceeded);
            }
            list.iter().try_fold(0_u64, |total, value| {
                total
                    .checked_add(actual_record_count(value, item, depth + 1)?)
                    .ok_or(AiDisclosureError::RecordLimitExceeded)
            })
        }
    }
}

fn actual_nested_record_count(
    value: &serde_json::Value,
    shape: &AiDisclosureShape,
    depth: usize,
) -> Result<u64, AiDisclosureError> {
    match shape {
        AiDisclosureShape::Scalar { .. } => Ok(0),
        AiDisclosureShape::Object { .. } | AiDisclosureShape::List { .. } => {
            actual_record_count(value, shape, depth)
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rule() -> AiDisclosureRule {
        AiDisclosureRule::exportable(DataClassification::Internal)
    }

    fn record() -> AiDisclosureShape {
        AiDisclosureShape::object(
            rule(),
            [("id".to_owned(), AiDisclosureShape::scalar(rule()))],
        )
    }

    fn schema(result: AiDisclosureShape) -> AiDisclosureSchema {
        AiDisclosureSchema::new(
            "records-v1",
            AiDisclosureShape::object(rule(), [("Records".to_owned(), result)]),
        )
        .expect("test disclosure")
    }

    #[test]
    fn sibling_lists_add_at_the_total_record_boundary() {
        let schema = schema(AiDisclosureShape::object(
            rule(),
            [
                ("id".to_owned(), AiDisclosureShape::scalar(rule())),
                (
                    "left".to_owned(),
                    AiDisclosureShape::list(rule(), 2, record()),
                ),
                (
                    "right".to_owned(),
                    AiDisclosureShape::list(rule(), 3, record()),
                ),
            ],
        ));
        assert_eq!(schema.maximum_graphql_record_count(), Some(6));
        let result = json!({
            "Records": {
                "id": "root",
                "left": [{"id":"l1"}, {"id":"l2"}],
                "right": [{"id":"r1"}, {"id":"r2"}, {"id":"r3"}]
            }
        });
        assert!(
            schema
                .evaluate_graphql_with_record_limit(&result, 6)
                .is_ok()
        );
        assert_eq!(
            schema.evaluate_graphql_with_record_limit(&result, 5),
            Err(AiDisclosureError::RecordLimitExceeded)
        );
    }

    #[test]
    fn nested_lists_multiply_at_the_total_record_boundary() {
        let nested = AiDisclosureShape::object(
            rule(),
            [
                ("id".to_owned(), AiDisclosureShape::scalar(rule())),
                (
                    "children".to_owned(),
                    AiDisclosureShape::list(rule(), 3, record()),
                ),
            ],
        );
        let schema = schema(AiDisclosureShape::list(rule(), 2, nested));
        assert_eq!(schema.maximum_graphql_record_count(), Some(8));
        let result = json!({
            "Records": [
                {"id":"p1", "children":[{"id":"1"},{"id":"2"},{"id":"3"}]},
                {"id":"p2", "children":[{"id":"4"},{"id":"5"},{"id":"6"}]}
            ]
        });
        assert!(
            schema
                .evaluate_graphql_with_record_limit(&result, 8)
                .is_ok()
        );
        assert_eq!(
            schema.evaluate_graphql_with_record_limit(&result, 7),
            Err(AiDisclosureError::RecordLimitExceeded)
        );
    }

    #[test]
    fn scalar_list_items_count_as_records_but_object_fields_do_not() {
        let schema = schema(AiDisclosureShape::list(
            rule(),
            3,
            AiDisclosureShape::scalar(rule()),
        ));
        assert_eq!(schema.maximum_graphql_record_count(), Some(3));
        assert!(
            schema
                .evaluate_graphql_with_record_limit(&json!({"Records":["a", "b", "c"]}), 3)
                .is_ok()
        );
    }
}
