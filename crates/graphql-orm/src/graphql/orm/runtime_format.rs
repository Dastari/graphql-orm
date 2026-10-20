//! Owned semantic format descriptors for stored runtime scalars.
//!
//! A [`RuntimeFieldFormat`] tells hosts and compiled GraphQL modules what a
//! stored scalar *means* (for example an external resource reference stored as
//! a UUID) without changing its value kind, physical type or nullability.
//! Descriptors are deterministic data: a validated namespace, a positive
//! version and a bounded payload without floating-point numbers. They contain
//! no callbacks or borrowed statics, and the ORM never interprets them beyond
//! validation and canonical identity.
//!
//! A descriptor participates in [`ValidatedRuntimeSchema::canonical_bytes`]
//! and therefore in [`ValidatedRuntimeSchema::fingerprint`] only when present.
//! It never affects the physical migration target, so adding, changing or
//! removing a descriptor is a new schema revision that requires no DDL.
//!
//! [`ValidatedRuntimeSchema::canonical_bytes`]: super::ValidatedRuntimeSchema::canonical_bytes
//! [`ValidatedRuntimeSchema::fingerprint`]: super::ValidatedRuntimeSchema::fingerprint

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU16;

/// Maximum namespace length in bytes.
pub const MAX_FORMAT_NAMESPACE_LEN: usize = 64;
/// Maximum canonical payload JSON in bytes.
pub const MAX_FORMAT_PAYLOAD_BYTES: usize = 4096;
/// Maximum payload nesting below the top-level object.
pub const MAX_FORMAT_PAYLOAD_DEPTH: usize = 8;
/// Maximum payload values, including containers.
pub const MAX_FORMAT_PAYLOAD_NODES: usize = 256;
/// Maximum bytes of one payload string.
pub const MAX_FORMAT_PAYLOAD_STRING_BYTES: usize = 1024;
/// Maximum bytes of one payload object key.
pub const MAX_FORMAT_PAYLOAD_KEY_LEN: usize = 64;
/// First namespace segment reserved for formats defined by this crate.
pub const RESERVED_FORMAT_NAMESPACE: &str = "graphql_orm";
const MAX_FORMAT_SEGMENT_LEN: usize = 32;

/// Value-free reason a format descriptor was rejected.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeFormatProblem {
    /// The namespace is not `segment(.segment)+` of `[a-z][a-z0-9_]*` segments.
    InvalidNamespace,
    /// The namespace uses the reserved `graphql_orm` segment.
    ReservedNamespace,
    /// The version is zero.
    InvalidVersion,
    /// The payload is not an object of supported JSON values.
    InvalidPayload,
    /// The payload contains a floating-point number.
    FloatInPayload,
    /// A payload key is not `[a-z][a-z0-9_]*` within the key bound.
    InvalidPayloadKey,
    /// The payload exceeds a byte, string or node bound.
    PayloadTooLarge,
    /// The payload exceeds [`MAX_FORMAT_PAYLOAD_DEPTH`].
    PayloadTooDeep,
}

impl RuntimeFormatProblem {
    /// Stable machine code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidNamespace => "invalid_namespace",
            Self::ReservedNamespace => "reserved_namespace",
            Self::InvalidVersion => "invalid_version",
            Self::InvalidPayload => "invalid_payload",
            Self::FloatInPayload => "float_in_payload",
            Self::InvalidPayloadKey => "invalid_payload_key",
            Self::PayloadTooLarge => "payload_too_large",
            Self::PayloadTooDeep => "payload_too_deep",
        }
    }
}

impl fmt::Display for RuntimeFormatProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Rejected descriptor. Formatting never includes submitted values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeFormatError {
    problem: RuntimeFormatProblem,
}

impl RuntimeFormatError {
    const fn new(problem: RuntimeFormatProblem) -> Self {
        Self { problem }
    }

    /// Value-free rejection reason.
    pub const fn problem(self) -> RuntimeFormatProblem {
        self.problem
    }
}

impl fmt::Display for RuntimeFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime field format rejected ({})", self.problem)
    }
}

impl std::error::Error for RuntimeFormatError {}

/// Validated descriptor namespace such as `example.file_ref`.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct RuntimeFormatNamespace(String);

impl RuntimeFormatNamespace {
    /// Validate a namespace of two or more lowercase segments.
    ///
    /// # Errors
    ///
    /// [`RuntimeFormatProblem::InvalidNamespace`] or
    /// [`RuntimeFormatProblem::ReservedNamespace`].
    pub fn new(value: impl Into<String>) -> Result<Self, RuntimeFormatError> {
        let value = value.into();
        check_namespace(&value)?;
        Ok(Self(value))
    }

    /// The namespace text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn check_namespace(value: &str) -> Result<(), RuntimeFormatError> {
    let segment_ok = |segment: &str| {
        let mut bytes = segment.bytes();
        segment.len() <= MAX_FORMAT_SEGMENT_LEN
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    };
    if value.len() > MAX_FORMAT_NAMESPACE_LEN
        || value.split('.').count() < 2
        || !value.split('.').all(segment_ok)
    {
        return Err(RuntimeFormatError::new(
            RuntimeFormatProblem::InvalidNamespace,
        ));
    }
    if value.split('.').next() == Some(RESERVED_FORMAT_NAMESPACE) {
        return Err(RuntimeFormatError::new(
            RuntimeFormatProblem::ReservedNamespace,
        ));
    }
    Ok(())
}

impl TryFrom<String> for RuntimeFormatNamespace {
    type Error = RuntimeFormatError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RuntimeFormatNamespace> for String {
    fn from(value: RuntimeFormatNamespace) -> Self {
        value.0
    }
}

impl fmt::Display for RuntimeFormatNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One payload value. Floating-point numbers are deliberately unsupported so
/// canonical identity never depends on float formatting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeFormatValue {
    /// JSON `null`.
    Null,
    /// Boolean.
    Boolean(bool),
    /// Signed 64-bit integer.
    Integer(i64),
    /// Unicode string.
    String(String),
    /// Ordered list.
    List(Vec<RuntimeFormatValue>),
    /// Object with sorted keys.
    Object(BTreeMap<String, RuntimeFormatValue>),
}

/// Bounded, canonical descriptor payload: a JSON object without floats.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "serde_json::Value", into = "serde_json::Value")]
pub struct RuntimeFormatPayload(BTreeMap<String, RuntimeFormatValue>);

impl RuntimeFormatPayload {
    /// Validate an owned payload object.
    ///
    /// # Errors
    ///
    /// The value-free [`RuntimeFormatProblem`] for an invalid key or value, or
    /// for exceeding a payload bound.
    pub fn new(entries: BTreeMap<String, RuntimeFormatValue>) -> Result<Self, RuntimeFormatError> {
        let payload = Self(entries);
        payload.check()?;
        Ok(payload)
    }

    /// The empty payload.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Whether the payload has no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Look up one top-level entry.
    pub fn get(&self, key: &str) -> Option<&RuntimeFormatValue> {
        self.0.get(key)
    }

    /// Top-level entries in key order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &RuntimeFormatValue)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }

    /// Canonical compact JSON with sorted keys.
    pub fn canonical_json(&self) -> String {
        let mut out = String::new();
        write_object(&self.0, &mut out);
        out
    }

    pub(super) fn check(&self) -> Result<(), RuntimeFormatError> {
        let mut nodes = 0usize;
        check_object(&self.0, 0, &mut nodes)?;
        if self.canonical_json().len() > MAX_FORMAT_PAYLOAD_BYTES {
            return Err(RuntimeFormatError::new(
                RuntimeFormatProblem::PayloadTooLarge,
            ));
        }
        Ok(())
    }
}

fn check_object(
    entries: &BTreeMap<String, RuntimeFormatValue>,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), RuntimeFormatError> {
    if depth > MAX_FORMAT_PAYLOAD_DEPTH {
        return Err(RuntimeFormatError::new(
            RuntimeFormatProblem::PayloadTooDeep,
        ));
    }
    for (key, value) in entries {
        let mut bytes = key.bytes();
        if key.len() > MAX_FORMAT_PAYLOAD_KEY_LEN
            || !bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            || !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(RuntimeFormatError::new(
                RuntimeFormatProblem::InvalidPayloadKey,
            ));
        }
        check_value(value, depth, nodes)?;
    }
    Ok(())
}

fn check_value(
    value: &RuntimeFormatValue,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), RuntimeFormatError> {
    *nodes += 1;
    if *nodes > MAX_FORMAT_PAYLOAD_NODES {
        return Err(RuntimeFormatError::new(
            RuntimeFormatProblem::PayloadTooLarge,
        ));
    }
    match value {
        RuntimeFormatValue::String(text) if text.len() > MAX_FORMAT_PAYLOAD_STRING_BYTES => Err(
            RuntimeFormatError::new(RuntimeFormatProblem::PayloadTooLarge),
        ),
        RuntimeFormatValue::List(values) => {
            if depth + 1 > MAX_FORMAT_PAYLOAD_DEPTH {
                return Err(RuntimeFormatError::new(
                    RuntimeFormatProblem::PayloadTooDeep,
                ));
            }
            values
                .iter()
                .try_for_each(|value| check_value(value, depth + 1, nodes))
        }
        RuntimeFormatValue::Object(entries) => check_object(entries, depth + 1, nodes),
        _ => Ok(()),
    }
}

fn write_object(entries: &BTreeMap<String, RuntimeFormatValue>, out: &mut String) {
    out.push('{');
    for (index, (key, value)) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        super::runtime_literal::write_json_string(key, out);
        out.push(':');
        write_value(value, out);
    }
    out.push('}');
}

fn write_value(value: &RuntimeFormatValue, out: &mut String) {
    match value {
        RuntimeFormatValue::Null => out.push_str("null"),
        RuntimeFormatValue::Boolean(value) => out.push_str(if *value { "true" } else { "false" }),
        RuntimeFormatValue::Integer(value) => out.push_str(&value.to_string()),
        RuntimeFormatValue::String(value) => super::runtime_literal::write_json_string(value, out),
        RuntimeFormatValue::List(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(value, out);
            }
            out.push(']');
        }
        RuntimeFormatValue::Object(entries) => write_object(entries, out),
    }
}

fn from_json(
    value: serde_json::Value,
    depth: usize,
) -> Result<RuntimeFormatValue, RuntimeFormatError> {
    if depth > MAX_FORMAT_PAYLOAD_DEPTH + 1 {
        return Err(RuntimeFormatError::new(
            RuntimeFormatProblem::PayloadTooDeep,
        ));
    }
    Ok(match value {
        serde_json::Value::Null => RuntimeFormatValue::Null,
        serde_json::Value::Bool(value) => RuntimeFormatValue::Boolean(value),
        serde_json::Value::Number(number) => match number.as_i64() {
            Some(value) => RuntimeFormatValue::Integer(value),
            None if number.is_f64() => {
                return Err(RuntimeFormatError::new(
                    RuntimeFormatProblem::FloatInPayload,
                ));
            }
            None => {
                return Err(RuntimeFormatError::new(
                    RuntimeFormatProblem::InvalidPayload,
                ));
            }
        },
        serde_json::Value::String(value) => RuntimeFormatValue::String(value),
        serde_json::Value::Array(values) => RuntimeFormatValue::List(
            values
                .into_iter()
                .map(|value| from_json(value, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        serde_json::Value::Object(entries) => RuntimeFormatValue::Object(
            entries
                .into_iter()
                .map(|(key, value)| Ok((key, from_json(value, depth + 1)?)))
                .collect::<Result<_, RuntimeFormatError>>()?,
        ),
    })
}

fn to_json(value: RuntimeFormatValue) -> serde_json::Value {
    match value {
        RuntimeFormatValue::Null => serde_json::Value::Null,
        RuntimeFormatValue::Boolean(value) => serde_json::Value::Bool(value),
        RuntimeFormatValue::Integer(value) => serde_json::Value::from(value),
        RuntimeFormatValue::String(value) => serde_json::Value::String(value),
        RuntimeFormatValue::List(values) => {
            serde_json::Value::Array(values.into_iter().map(to_json).collect())
        }
        RuntimeFormatValue::Object(entries) => serde_json::Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key, to_json(value)))
                .collect(),
        ),
    }
}

impl TryFrom<serde_json::Value> for RuntimeFormatPayload {
    type Error = RuntimeFormatError;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        match from_json(value, 0)? {
            RuntimeFormatValue::Object(entries) => Self::new(entries),
            _ => Err(RuntimeFormatError::new(
                RuntimeFormatProblem::InvalidPayload,
            )),
        }
    }
}

impl From<RuntimeFormatPayload> for serde_json::Value {
    fn from(value: RuntimeFormatPayload) -> Self {
        to_json(RuntimeFormatValue::Object(value.0))
    }
}

/// Owned semantic descriptor of a stored scalar field.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFieldFormat {
    /// Owner-defined namespace.
    pub namespace: RuntimeFormatNamespace,
    /// Positive descriptor version within the namespace.
    pub version: NonZeroU16,
    /// Bounded canonical payload; omitted from serialization when empty.
    #[serde(default, skip_serializing_if = "RuntimeFormatPayload::is_empty")]
    pub payload: RuntimeFormatPayload,
}

impl RuntimeFieldFormat {
    /// Construct a descriptor from validated parts.
    ///
    /// # Errors
    ///
    /// [`RuntimeFormatProblem::InvalidVersion`] when `version` is zero.
    pub fn new(
        namespace: RuntimeFormatNamespace,
        version: u16,
        payload: RuntimeFormatPayload,
    ) -> Result<Self, RuntimeFormatError> {
        let version = NonZeroU16::new(version).ok_or(RuntimeFormatError::new(
            RuntimeFormatProblem::InvalidVersion,
        ))?;
        Ok(Self {
            namespace,
            version,
            payload,
        })
    }

    /// Re-check every bound; constructors and deserialization already enforce them.
    ///
    /// # Errors
    ///
    /// The value-free [`RuntimeFormatProblem`].
    pub fn validate(&self) -> Result<(), RuntimeFormatError> {
        check_namespace(self.namespace.as_str())?;
        self.payload.check()
    }

    /// Canonical `namespace@version:payload` identity text.
    pub fn canonical(&self) -> String {
        format!(
            "{}@{}:{}",
            self.namespace,
            self.version,
            self.payload.canonical_json()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RuntimeFormatProblem as P;

    #[test]
    fn namespaces_are_bounded_lowercase_segments() {
        for ok in ["example.file_ref", "a.b", "acme.v2.select"] {
            assert!(RuntimeFormatNamespace::new(ok).is_ok(), "{ok}");
        }
        for (bad, problem) in [
            ("file", P::InvalidNamespace),
            ("Example.file", P::InvalidNamespace),
            ("example..file", P::InvalidNamespace),
            ("example.1file", P::InvalidNamespace),
            ("example.file-ref", P::InvalidNamespace),
            ("graphql_orm.email", P::ReservedNamespace),
        ] {
            assert_eq!(
                RuntimeFormatNamespace::new(bad).unwrap_err().problem(),
                problem,
                "{bad}"
            );
        }
        let long = format!("a.{}", "b".repeat(MAX_FORMAT_NAMESPACE_LEN));
        assert!(RuntimeFormatNamespace::new(long).is_err());
    }

    #[test]
    fn payloads_round_trip_canonically_and_reject_floats() {
        let json = serde_json::json!({"z": [1, "two", null], "a": {"nested": true}});
        let payload = RuntimeFormatPayload::try_from(json.clone()).unwrap();
        assert_eq!(
            payload.canonical_json(),
            r#"{"a":{"nested":true},"z":[1,"two",null]}"#
        );
        assert_eq!(serde_json::Value::from(payload.clone()), json);
        let text = serde_json::to_string(&payload).unwrap();
        assert_eq!(text, r#"{"a":{"nested":true},"z":[1,"two",null]}"#);
        assert_eq!(
            serde_json::from_str::<RuntimeFormatPayload>(&text).unwrap(),
            payload
        );
        for (value, problem) in [
            (serde_json::json!({"a": 1.5}), P::FloatInPayload),
            (serde_json::json!({"a": u64::MAX}), P::InvalidPayload),
            (serde_json::json!([1]), P::InvalidPayload),
            (serde_json::json!({"Bad": 1}), P::InvalidPayloadKey),
            (
                serde_json::json!({"a": "x".repeat(MAX_FORMAT_PAYLOAD_STRING_BYTES + 1)}),
                P::PayloadTooLarge,
            ),
        ] {
            assert_eq!(
                RuntimeFormatPayload::try_from(value).unwrap_err().problem(),
                problem
            );
        }
        let mut deep = serde_json::json!(1);
        for _ in 0..=MAX_FORMAT_PAYLOAD_DEPTH + 1 {
            deep = serde_json::json!({ "a": deep });
        }
        assert_eq!(
            RuntimeFormatPayload::try_from(deep).unwrap_err().problem(),
            P::PayloadTooDeep
        );
        let wide = serde_json::Value::Object(
            (0..=MAX_FORMAT_PAYLOAD_NODES)
                .map(|index| (format!("k{index}"), serde_json::json!(index)))
                .collect(),
        );
        assert_eq!(
            RuntimeFormatPayload::try_from(wide).unwrap_err().problem(),
            P::PayloadTooLarge
        );
    }

    #[test]
    fn descriptors_serialize_without_empty_payload_and_reject_zero_versions() {
        let format = RuntimeFieldFormat::new(
            RuntimeFormatNamespace::new("example.email").unwrap(),
            1,
            RuntimeFormatPayload::empty(),
        )
        .unwrap();
        let text = serde_json::to_string(&format).unwrap();
        assert_eq!(text, r#"{"namespace":"example.email","version":1}"#);
        assert_eq!(
            serde_json::from_str::<RuntimeFieldFormat>(&text).unwrap(),
            format
        );
        assert_eq!(format.canonical(), "example.email@1:{}");
        assert!(
            serde_json::from_str::<RuntimeFieldFormat>(
                r#"{"namespace":"example.email","version":0}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<RuntimeFieldFormat>(
                r#"{"namespace":"example.email","version":1,"extra":1}"#
            )
            .is_err()
        );
        assert_eq!(
            RuntimeFieldFormat::new(
                RuntimeFormatNamespace::new("example.email").unwrap(),
                0,
                RuntimeFormatPayload::empty()
            )
            .unwrap_err()
            .problem(),
            P::InvalidVersion
        );
    }
}
