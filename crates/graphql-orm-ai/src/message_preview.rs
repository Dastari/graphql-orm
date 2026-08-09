//! Canonical protected-message preview representation and compatibility reads.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use serde_json::Value;

use crate::AiError;

pub(crate) fn canonical_message_preview(text: &str) -> Value {
    Value::String(text.to_owned())
}

pub(crate) fn decode_message_preview(
    value: Value,
    maximum_bytes: usize,
) -> Result<String, AiError> {
    let text = match value {
        Value::String(text) => text,
        Value::Object(mut object) if object.len() == 1 => match object.remove("text") {
            Some(Value::String(text)) => text,
            _ => return Err(AiError::PersistenceFailed),
        },
        _ => return Err(AiError::PersistenceFailed),
    };
    if text.len() > maximum_bytes {
        return Err(AiError::PersistenceFailed);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn canonical_and_exact_legacy_previews_decode_with_utf8_byte_bounds() {
        assert_eq!(
            decode_message_preview(canonical_message_preview("éé"), 4)
                .expect("canonical preview should decode"),
            "éé"
        );
        assert_eq!(
            decode_message_preview(json!({"text": "legacy"}), 6)
                .expect("exact legacy preview should decode"),
            "legacy"
        );
        assert!(matches!(
            decode_message_preview(json!("ééa"), 4),
            Err(AiError::PersistenceFailed)
        ));
        assert!(matches!(
            decode_message_preview(json!({"text": "legacy!"}), 6),
            Err(AiError::PersistenceFailed)
        ));
    }

    #[test]
    fn malformed_or_ambiguous_legacy_previews_fail_closed() {
        for value in [
            Value::Null,
            json!(7),
            json!([]),
            json!({}),
            json!({"text": null}),
            json!({"text": "preview", "extra": true}),
            json!({"preview": "wrong field"}),
        ] {
            assert!(matches!(
                decode_message_preview(value, 4_096),
                Err(AiError::PersistenceFailed)
            ));
        }
    }
}
