//! Public descriptions, classification and export disposition of runtime entities.
//!
//! A runtime collection, field or relation may carry one optional public
//! description. It is the only description: runtime GraphQL emits it in SDL and
//! introspection, and the runtime semantic catalogue publishes the same text to
//! AI capability indexes. There is no separate AI-only description.
//!
//! Absent members default exactly as the static derives do:
//!
//! - descriptions fall back to the humanized API name (`createdAt` becomes
//!   `Created At`);
//! - a collection's classification falls back to
//!   [`GraphqlSemanticClassification::Internal`];
//! - a field or relation inherits its collection's classification and may only
//!   strengthen it;
//! - export falls back to [`GraphqlSemanticExport::Exportable`], except that a
//!   `secret` member is always [`GraphqlSemanticExport::NeverExport`].
//!
//! The members are semantic, not structural. Present members join
//! [`ValidatedRuntimeSchema::canonical_bytes`] and therefore
//! [`ValidatedRuntimeSchema::fingerprint`]; they never affect
//! [`ValidatedRuntimeSchema::structural_fingerprint`], the physical migration
//! target, plan hashes or DDL. Absent members are omitted from serialization,
//! so schemas that do not use them keep their exact earlier identities.
//!
//! [`ValidatedRuntimeSchema::canonical_bytes`]: super::ValidatedRuntimeSchema::canonical_bytes
//! [`ValidatedRuntimeSchema::fingerprint`]: super::ValidatedRuntimeSchema::fingerprint
//! [`ValidatedRuntimeSchema::structural_fingerprint`]: super::ValidatedRuntimeSchema::structural_fingerprint

use super::{
    GraphqlSemanticClassification, GraphqlSemanticExport, RuntimeCollection, RuntimeField,
    RuntimeRelation,
};

/// Static-derive fallback description for an identifier.
pub(crate) fn humanize_identifier(value: &str) -> String {
    use convert_case::{Case, Casing};
    let words = value.to_case(Case::Title);
    if words.is_empty() {
        "GraphQL value".to_owned()
    } else {
        words
    }
}

fn effective_export(
    declared: Option<GraphqlSemanticExport>,
    classification: GraphqlSemanticClassification,
) -> GraphqlSemanticExport {
    match declared {
        Some(export) => export,
        None if classification == GraphqlSemanticClassification::Secret => {
            GraphqlSemanticExport::NeverExport
        }
        None => GraphqlSemanticExport::Exportable,
    }
}

impl RuntimeCollection {
    /// The declared description, else the humanized `api_type_name`.
    pub fn effective_description(&self) -> String {
        self.description
            .clone()
            .unwrap_or_else(|| humanize_identifier(&self.api_type_name))
    }

    /// The declared classification, else `internal`.
    pub fn effective_classification(&self) -> GraphqlSemanticClassification {
        self.classification.unwrap_or_default()
    }
}

impl RuntimeField {
    /// The declared description, else the humanized `api_name`.
    pub fn effective_description(&self) -> String {
        self.description
            .clone()
            .unwrap_or_else(|| humanize_identifier(&self.api_name))
    }

    /// The declared classification, else the collection's.
    pub fn effective_classification(
        &self,
        collection: &RuntimeCollection,
    ) -> GraphqlSemanticClassification {
        self.classification
            .unwrap_or_else(|| collection.effective_classification())
    }

    /// The declared export, else `never_export` for secret fields and
    /// `exportable` otherwise.
    pub fn effective_export(&self, collection: &RuntimeCollection) -> GraphqlSemanticExport {
        effective_export(self.export, self.effective_classification(collection))
    }
}

impl RuntimeRelation {
    /// The declared description, else the humanized `api_name`.
    pub fn effective_description(&self) -> String {
        self.description
            .clone()
            .unwrap_or_else(|| humanize_identifier(&self.api_name))
    }

    /// The declared classification, else the declaring collection's.
    pub fn effective_classification(
        &self,
        collection: &RuntimeCollection,
    ) -> GraphqlSemanticClassification {
        self.classification
            .unwrap_or_else(|| collection.effective_classification())
    }

    /// The declared export, else `never_export` for secret relations and
    /// `exportable` otherwise.
    pub fn effective_export(&self, collection: &RuntimeCollection) -> GraphqlSemanticExport {
        effective_export(self.export, self.effective_classification(collection))
    }
}

/// Why a member's classification or export is invalid.
pub(crate) fn classification_problem(
    collection: &RuntimeCollection,
    classification: GraphqlSemanticClassification,
    export: Option<GraphqlSemanticExport>,
) -> Option<&'static str> {
    if classification < collection.effective_classification() {
        Some("classification is weaker than the collection classification")
    } else if classification == GraphqlSemanticClassification::Secret
        && export == Some(GraphqlSemanticExport::Exportable)
    {
        Some("a secret member cannot be exportable")
    } else {
        None
    }
}

/// Stable lowercase token of a semantic enum, as serialized.
pub(crate) fn semantic_token<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(token)) => token,
        _ => unreachable!("semantic enums serialize as strings"),
    }
}

#[cfg(test)]
mod tests {
    use super::humanize_identifier;

    #[test]
    fn fallbacks_match_the_static_derive() {
        assert_eq!(humanize_identifier("createdAt"), "Created At");
        assert_eq!(humanize_identifier("created_at"), "Created At");
        assert_eq!(humanize_identifier("Invoice"), "Invoice");
        assert_eq!(humanize_identifier("LineItem"), "Line Item");
        assert_eq!(humanize_identifier(""), "GraphQL value");
    }
}
