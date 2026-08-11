//! Canonical JSON encoding for stable profile fingerprints.

use serde::Serialize;
use serde_json::{Map, Value};

pub(crate) fn canonical_json_bytes<T: Serialize>(value: &T) -> Vec<u8> {
    let value = serde_json::to_value(value).expect("fingerprint value always serializes");
    serde_json::to_vec(&canonical_json_value(value))
        .expect("canonical fingerprint value always serializes")
}

pub(crate) fn canonical_json_value(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let mut canonical = Map::new();
            for (key, value) in entries {
                canonical.insert(key, canonical_json_value(value));
            }
            Value::Object(canonical)
        }
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonical_json_value).collect())
        }
        scalar => scalar,
    }
}
