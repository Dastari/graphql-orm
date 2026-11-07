//! Versioned, opt-in bindings for exact private receiver requests.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Additional signed claim key. Existing operation bindings are unchanged.
pub const RECEIVER_PURPOSE_CLAIM: &str = "graphql_orm_receiver_purpose";

/// Static receiver-owned purpose contract. All identifiers are host-defined.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverPurposeMetadata {
    /// Intended service identity.
    pub receiver: String,
    /// Logical execution target identity.
    pub target: String,
    /// Exact schema revision/fingerprint.
    pub schema: String,
    /// Registered capability identity/fingerprint.
    pub capability: String,
    /// Registered projection fingerprint.
    pub projection: String,
    /// Registered disclosure fingerprint.
    pub disclosure: String,
    /// Bound resource class.
    pub resource_type: String,
    /// Bound resource identity.
    pub resource_id: String,
}

/// Version 1 authenticated purpose. Unknown versions must be refused by receivers.
/// Hashes use exact document bytes and recursively key-sorted JSON respectively.
/// Arrays retain order; absent keys, nulls and explicit defaults remain distinct.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverPurposeV1 {
    /// Format version; exactly 1.
    pub version: u32,
    /// Target class; exactly `private_direct` for this protocol.
    pub target_class: String,
    /// Static contract metadata.
    pub metadata: ReceiverPurposeMetadata,
    /// Named query.
    pub operation_name: String,
    /// Exact UTF-8 document SHA-256, lowercase hex without a prefix.
    pub document_sha256: String,
    /// Raw variables JSON binding, before GraphQL defaults/coercion.
    pub variables_sha256: String,
    /// Coerced root argument object binding.
    pub arguments_sha256: String,
}

impl ReceiverPurposeV1 {
    /// Binds one issuance request. Receivers independently recompute these values.
    /// This does not authorize issuance or validate a GraphQL schema.
    pub fn bind(
        metadata: ReceiverPurposeMetadata,
        operation_name: &str,
        document: &str,
        variables: &Value,
        arguments: &Value,
    ) -> Self {
        Self {
            version: 1,
            target_class: "private_direct".into(),
            metadata,
            operation_name: operation_name.into(),
            document_sha256: exact_document_hash(document),
            variables_sha256: canonical_json_hash(variables),
            arguments_sha256: canonical_json_hash(arguments),
        }
    }
}

/// SHA-256 of exact document bytes; never parses or normalizes input.
pub fn exact_document_hash(document: &str) -> String {
    format!("{:x}", Sha256::digest(document.as_bytes()))
}

/// SHA-256 of compact JSON with recursively sorted object keys.
/// This preserves the existing private-remote JSON binding format, including
/// serde_json number encoding. It is not RFC 8785 and does not coerce values.
pub fn canonical_json_hash(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), sorted(v)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => Value::Array(values.iter().map(sorted).collect()),
            value => value.clone(),
        }
    }
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&sorted(value)).expect("JSON value serializes"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn golden_bindings_preserve_raw_semantics() {
        assert_eq!(
            exact_document_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            canonical_json_hash(&serde_json::json!({})),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        assert_eq!(
            canonical_json_hash(&serde_json::json!({"b":2,"a":1})),
            "43258cff783fe7036d8a43033f830adfc60ec037382473548ac742b888292777"
        );
        for (raw, golden) in [
            (
                r#"{"x":null}"#,
                "c6b8df5aba33a39cbdee46ffaf77fae93ea2aa7d99d66408162b05a42105bd71",
            ),
            (
                r#"{"x":3}"#,
                "54afd0d590e6b277d9cd1ce46e5480f683682e43b6b480f7d24906d9e935e44c",
            ),
            (
                r#"{"x":[1,2]}"#,
                "036b898f9248c0e83d645e262473d1d93b3085400b881ff658928b81ca06de91",
            ),
            (
                r#"{"x":[2,1]}"#,
                "65021519edb350469a6321dcf08b0bb84511688a421a6109ecfda65f5bc759c9",
            ),
            (
                r#"{"a":{"b":2,"z":1}}"#,
                "790e419d00076a2e68601e7515628f0439349bfbe53c111fc3a29759f7f3c858",
            ),
        ] {
            assert_eq!(
                canonical_json_hash(&serde_json::from_str::<Value>(raw).unwrap()),
                golden
            );
        }
        let cases = [
            serde_json::json!({}),
            serde_json::json!({"x":null}),
            serde_json::json!({"x":3}),
            serde_json::json!({"x":[1,2]}),
            serde_json::json!({"x":[2,1]}),
        ];
        let hashes: std::collections::BTreeSet<_> = cases.iter().map(canonical_json_hash).collect();
        assert_eq!(hashes.len(), cases.len());
    }
}
