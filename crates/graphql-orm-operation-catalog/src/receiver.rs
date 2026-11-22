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
///
/// Two documents that differ in whitespace, a comment, a line ending, field
/// order or Unicode normalization form have different hashes, although a
/// GraphQL server would execute them alike. This fails closed: an issuer must
/// hash the same bytes the receiver has registered.
pub fn exact_document_hash(document: &str) -> String {
    format!("{:x}", Sha256::digest(document.as_bytes()))
}

/// SHA-256 of compact JSON with recursively sorted object keys.
/// This preserves the existing private-remote JSON binding format, including
/// serde_json number encoding. It is not RFC 8785 and does not coerce values.
///
/// The issuer and the receiver must therefore both encode numbers as
/// `serde_json` does. The bytes differ from RFC 8785 for a number with a
/// fraction or exponent (`1.0` here, `1` there), for an integer above 2^53 in
/// magnitude, and for object keys whose UTF-16 and UTF-8 orders differ.
/// [`crate::rfc8785_json_hash`] is the RFC 8785 hash for new formats; this
/// function's output never changes.
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

    #[test]
    fn document_hash_does_not_normalize() {
        let documents = [
            "query Read { item { id } }",
            "query Read { item { id } }\n",
            "query Read {item{id}}",
            "query Read { item { id } } # note",
            "query Read {\r\n  item { id }\r\n}",
            "query Read {\n  item { id }\n}",
        ];
        let hashes: std::collections::BTreeSet<_> = documents
            .iter()
            .map(|document| exact_document_hash(document))
            .collect();
        assert_eq!(hashes.len(), documents.len());
        // Composed and decomposed forms of one visible character differ too.
        assert_ne!(
            exact_document_hash("\u{e9}"),
            exact_document_hash("e\u{301}")
        );
    }

    /// The version 1 binding keeps `serde_json` number spelling. Where that
    /// differs from RFC 8785 the two hashes differ, and where it does not
    /// they agree.
    #[test]
    fn binding_hash_is_pinned_and_is_not_rfc_8785_for_fractional_numbers() {
        let fractional = serde_json::json!({"limit": 1.0, "ratio": 2.5e-7});
        assert_eq!(
            canonical_json_hash(&fractional),
            format!("{:x}", Sha256::digest(br#"{"limit":1.0,"ratio":2.5e-7}"#))
        );
        assert_eq!(
            crate::rfc8785_json_hash(&fractional).unwrap(),
            format!("{:x}", Sha256::digest(br#"{"limit":1,"ratio":2.5e-7}"#))
        );
        assert_ne!(
            canonical_json_hash(&fractional),
            crate::rfc8785_json_hash(&fractional).unwrap()
        );
        for same in [
            serde_json::json!({"b": 2, "a": [1, "x", null, true]}),
            serde_json::json!({"id": "7", "page": {"limit": 50}}),
        ] {
            assert_eq!(
                canonical_json_hash(&same),
                crate::rfc8785_json_hash(&same).unwrap()
            );
        }
    }

    /// The exact bytes the version 1 binding hashes for numbers. A
    /// `serde_json` release that spells any of them differently changes every
    /// binding that holds such a number, and must not be adopted silently.
    #[test]
    fn binding_number_spelling_is_pinned() {
        let spelled = |value: serde_json::Value| serde_json::to_string(&value).unwrap();
        let actual = [
            spelled(serde_json::json!(0)),
            spelled(serde_json::json!(-7)),
            spelled(serde_json::json!(u64::MAX)),
            spelled(serde_json::json!(i64::MIN)),
            spelled(serde_json::json!(1.0)),
            spelled(serde_json::json!(-0.0)),
            spelled(serde_json::json!(0.1)),
            spelled(serde_json::json!(2.5e-7)),
            spelled(serde_json::json!(1e15)),
            spelled(serde_json::json!(1e16)),
            spelled(serde_json::json!(1e21)),
            spelled(serde_json::json!(1.5e300)),
            spelled(serde_json::json!(5e-324)),
        ];
        assert_eq!(
            actual,
            [
                "0",
                "-7",
                "18446744073709551615",
                "-9223372036854775808",
                "1.0",
                "-0.0",
                "0.1",
                "2.5e-7",
                "1000000000000000.0",
                "1e+16",
                "1e+21",
                "1.5e+300",
                "5e-324",
            ]
        );
    }

    #[test]
    fn purpose_binding_distinguishes_absent_null_and_reordered_values() {
        let metadata = || ReceiverPurposeMetadata {
            receiver: "receiver".into(),
            target: "target".into(),
            schema: "schema".into(),
            capability: "capability".into(),
            projection: "projection".into(),
            disclosure: "disclosure".into(),
            resource_type: "item".into(),
            resource_id: "1".into(),
        };
        let bind = |variables: serde_json::Value| {
            ReceiverPurposeV1::bind(
                metadata(),
                "Read",
                "query Read { item }",
                &variables,
                &variables,
            )
        };
        let base = bind(serde_json::json!({"a": 1, "b": [1, 2]}));
        assert_eq!(base.version, 1);
        assert_eq!(base.target_class, "private_direct");
        assert_eq!(base, bind(serde_json::json!({"b": [1, 2], "a": 1})));
        for other in [
            serde_json::json!({"a": 1, "b": [2, 1]}),
            serde_json::json!({"a": 1, "b": [1, 2], "c": null}),
            serde_json::json!({"a": null, "b": [1, 2]}),
            serde_json::json!({"a": "1", "b": [1, 2]}),
        ] {
            assert_ne!(base.variables_sha256, bind(other).variables_sha256);
        }
        // Unknown members and versions are for the receiver to refuse.
        let mut encoded = serde_json::to_value(&base).unwrap();
        encoded["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ReceiverPurposeV1>(encoded).is_err());
    }
}
