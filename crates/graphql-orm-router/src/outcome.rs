//! The `extensions.outcome` value the router adds, when configured, to each
//! error of an operation it refused before sending any subgraph request.
//!
//! The vocabulary is the one generated ORM services put on mutation errors:
//! `refused`, `failed`, `committed` and `unknown`. The router depends on no ORM
//! package, so it keeps its own literals; the shared table in
//! `docs/reference/graphql-orm/error-codes.md` maps the two. The router only
//! ever adds `refused`, and never replaces an `outcome` already present, so a
//! subgraph's own value reaches the client unchanged.

use hive_router::{GraphQLError, sonic_rs};
use serde_json::{Map, Value};

/// The extension key, beside `code`.
pub(crate) const OUTCOME_EXTENSION: &str = "outcome";

/// Nothing ran: the router refused the operation before any subgraph request.
pub(crate) const REFUSED: &str = "refused";

/// Codes that Hive 0.2.19 sends in a `graphql-transport-ws` `error` frame only
/// while it prepares, parses, validates, normalizes or coerces an operation,
/// before it plans or executes it. Its WebSocket server reports every such
/// pipeline failure as one error with the pipeline code, so validation always
/// arrives as `GRAPHQL_VALIDATION_FAILED` there. Execution failures
/// (`PLAN_EXECUTION_FAILED`), the request timeout (`GATEWAY_TIMEOUT`), a
/// schema reload and every code raised after execution are deliberately
/// absent: an absent outcome means unknown. Review this list whenever the
/// pinned Hive version changes.
const HIVE_WEBSOCKET_REFUSAL_CODES: &[&str] = &[
    "BAD_REQUEST",
    "BAD_USER_INPUT",
    "GRAPHQL_PARSE_FAILED",
    "GRAPHQL_VALIDATION_FAILED",
    "INTROSPECTION_DISABLED",
    "OPERATION_RESOLUTION_FAILURE",
    "PERSISTED_DOCUMENT_EXTRACTION_FAILED",
    "PERSISTED_DOCUMENT_ID_REQUIRED",
    "PERSISTED_DOCUMENT_NOT_FOUND",
    "SUBSCRIPTIONS_NOT_SUPPORTED",
];

/// Hive's request timeout covers execution, so it is never a refusal, even
/// when it fires before the execute stage.
pub(crate) const GATEWAY_TIMEOUT: &str = "GATEWAY_TIMEOUT";

/// Adds `outcome: "refused"` to an engine error, keeping any outcome present.
pub(crate) fn mark_refused(error: &mut GraphQLError) {
    if error.extensions.get(OUTCOME_EXTENSION).is_none() {
        error
            .extensions
            .set(OUTCOME_EXTENSION.to_owned(), sonic_rs::Value::from(REFUSED));
    }
}

/// Returns `error`, marked refused when the router is configured to say so.
pub(crate) fn refusal(mut error: GraphQLError, enabled: bool) -> GraphQLError {
    if enabled {
        mark_refused(&mut error);
    }
    error
}

/// Adds `outcome: "refused"` to every error of a `graphql-transport-ws`
/// `error` frame, keeping any outcome present.
pub(crate) fn mark_refused_frame(message: &mut Value) {
    let Some(errors) = message.get_mut("payload").and_then(Value::as_array_mut) else {
        return;
    };
    for error in errors {
        let Some(error) = error.as_object_mut() else {
            continue;
        };
        let extensions = error
            .entry("extensions")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(extensions) = extensions.as_object_mut() {
            extensions
                .entry(OUTCOME_EXTENSION)
                .or_insert_with(|| Value::from(REFUSED));
        }
    }
}

/// Marks a Hive `error` frame refused when every error in it carries a code
/// Hive raises only before planning or execution.
pub(crate) fn mark_hive_websocket_refusal(message: &mut Value) {
    let refused = message
        .get("payload")
        .and_then(Value::as_array)
        .is_some_and(|errors| {
            !errors.is_empty()
                && errors.iter().all(|error| {
                    error
                        .pointer("/extensions/code")
                        .and_then(Value::as_str)
                        .is_some_and(|code| HIVE_WEBSOCKET_REFUSAL_CODES.contains(&code))
                })
        });
    if refused {
        mark_refused_frame(message);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn engine_errors_keep_an_outcome_already_present() {
        let error = refusal(
            GraphQLError::from_message_and_code("refused", "FORBIDDEN"),
            true,
        );
        assert_eq!(
            serde_json::to_value(&error).unwrap()["extensions"],
            json!({"code": "FORBIDDEN", "outcome": "refused"})
        );
        let mut failed = GraphQLError::from_message_and_code("failed", "CONFLICT");
        failed.extensions.set(
            OUTCOME_EXTENSION.to_owned(),
            sonic_rs::Value::from("failed"),
        );
        let failed = refusal(failed, true);
        assert_eq!(
            serde_json::to_value(&failed).unwrap()["extensions"]["outcome"],
            "failed"
        );
        let disabled = refusal(
            GraphQLError::from_message_and_code("refused", "FORBIDDEN"),
            false,
        );
        assert!(
            serde_json::to_value(&disabled).unwrap()["extensions"]
                .get(OUTCOME_EXTENSION)
                .is_none()
        );
    }

    #[test]
    fn hive_websocket_frames_are_refused_only_for_pre_execution_codes() {
        let frame = |codes: &[&str]| {
            json!({"id": "1", "type": "error", "payload": codes
                .iter()
                .map(|code| json!({"message": "m", "extensions": {"code": code}}))
                .collect::<Vec<_>>()})
        };
        for code in HIVE_WEBSOCKET_REFUSAL_CODES {
            let mut message = frame(&[code]);
            mark_hive_websocket_refusal(&mut message);
            assert_eq!(
                message["payload"][0]["extensions"]["outcome"], REFUSED,
                "{code}"
            );
        }
        for codes in [
            &["PLAN_EXECUTION_FAILED"][..],
            &[GATEWAY_TIMEOUT],
            &["SUBSCRIPTION_SCHEMA_RELOAD"],
            &["SERVICE_UNAVAILABLE"],
            &["GRAPHQL_PARSE_FAILED", "PLAN_EXECUTION_FAILED"],
            &[],
        ] {
            let mut message = frame(codes);
            let before = message.clone();
            mark_hive_websocket_refusal(&mut message);
            assert_eq!(message, before, "{codes:?}");
        }
        let mut uncoded = json!({"type": "error", "payload": [{"message": "m"}]});
        let before = uncoded.clone();
        mark_hive_websocket_refusal(&mut uncoded);
        assert_eq!(uncoded, before);
    }

    #[test]
    fn router_frames_keep_an_outcome_already_present() {
        let mut message = json!({"type": "error", "payload": [
            {"message": "a", "extensions": {"code": "FORBIDDEN"}},
            {"message": "b", "extensions": {"code": "FORBIDDEN", "outcome": "committed"}},
            {"message": "c"}
        ]});
        mark_refused_frame(&mut message);
        assert_eq!(message["payload"][0]["extensions"]["outcome"], REFUSED);
        assert_eq!(message["payload"][1]["extensions"]["outcome"], "committed");
        assert_eq!(message["payload"][2]["extensions"]["outcome"], REFUSED);
    }
}
