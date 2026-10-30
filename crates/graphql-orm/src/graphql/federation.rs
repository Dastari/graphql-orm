//! Federation SDL helpers for generated GraphQL types.

#![allow(missing_docs)]

/// Standard Federation `@authenticated`, addressed through the default
/// namespace of async-graphql's existing Federation v2.5 link.
///
/// async-graphql 7.2.1 does not import or model `@authenticated` directly. A
/// non-imported Federation directive remains available under the standard
/// `federation__` namespace, so generated fields use
/// `@federation__authenticated`. Federation composition recognizes that name
/// as the standard directive; this is not a project-owned composed directive.
#[async_graphql::TypeDirective(
    name = "federation__authenticated",
    location = "FieldDefinition",
    location = "Object",
    location = "Interface",
    location = "Enum"
)]
pub fn federation_authenticated() {}

/// Standard Federation `@requiresScopes` addressed through the existing
/// Federation link's default namespace for generated subscription fields.
///
/// async-graphql 7.2.1 exposes a dedicated `requires_scopes` attribute for
/// object fields but not subscription fields. The namespaced form preserves
/// the same standard Federation identity without defining a project-owned
/// authorization directive.
#[async_graphql::TypeDirective(name = "federation__requiresScopes", location = "FieldDefinition")]
pub fn federation_requires_scopes(scopes: Vec<Vec<String>>) {}

/// Witness that generated Federation entity resolvers exist for an entity.
///
/// `#[graphql_entity(federation_key)]` is read by two derives. `GraphQLEntity`
/// records the declaration, while `GraphQLOperations` is the derive that owns
/// the `{Entity}Queries` object where an `#[graphql(entity)]` resolver — the
/// only construct async-graphql turns into a resolvable `@key` — can be
/// emitted. Without this witness, declaring a key on a type that lacks
/// `GraphQLOperations` would silently export no key at all. `GraphQLEntity`
/// therefore requires this trait, and only the operations derive implements it.
#[diagnostic::on_unimplemented(
    message = "`{Self}` declares `federation_key` but generates no Federation entity resolver",
    label = "no entity resolver is generated for `{Self}`",
    note = "add `GraphQLOperations` to this type's derive list; only that derive emits the generated queries object whose entity resolver async-graphql turns into a resolvable `@key`"
)]
pub trait GeneratedFederationEntityKeys {
    /// Number of `@key` directives generated for this entity.
    const FEDERATION_KEY_COUNT: usize;
}

/// Name of the generated global identity field and of its Federation `@key`.
pub const NODE_ID_FIELD: &str = "NodeId";

/// Encode a global record identity: base64url without padding of the UTF-8
/// bytes of the compact JSON array `[typename, ...keyParts]`.
///
/// Key parts are the primary-key values in primary-key order, each as its JSON
/// wire value. Non-ASCII characters are not escaped, so the result equals the
/// encoding `JSON.stringify` produces for the same array.
pub fn encode_node_id(typename: &str, key_parts: Vec<serde_json::Value>) -> async_graphql::ID {
    use base64::Engine as _;
    let mut array = Vec::with_capacity(key_parts.len() + 1);
    array.push(serde_json::Value::String(typename.to_owned()));
    array.extend(key_parts);
    let json = serde_json::to_vec(&serde_json::Value::Array(array))
        .expect("a JSON array of strings, numbers and booleans always serializes");
    async_graphql::ID(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
}

/// Decode a global record identity for `typename` into its key parts.
///
/// Returns `None` for anything that is not exactly an encoding of
/// `[typename, ...keyParts]` with `expected_parts` key parts: invalid base64url
/// (padding included), invalid UTF-8 or JSON, a non-canonical encoding, or a
/// different typename. Callers resolve `None` to null, so a malformed or
/// mismatched identity never reveals whether a row exists.
pub fn decode_node_id(
    node_id: &str,
    typename: &str,
    expected_parts: usize,
) -> Option<Vec<serde_json::Value>> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(node_id)
        .ok()?;
    let serde_json::Value::Array(mut array) = serde_json::from_slice(&bytes).ok()? else {
        return None;
    };
    if array.len() != expected_parts + 1
        || array.first().and_then(serde_json::Value::as_str) != Some(typename)
    {
        return None;
    }
    let key_parts = array.split_off(1);
    // Only the canonical spelling identifies a record; whitespace, escaped
    // non-ASCII or another number spelling would be a second id for one row.
    if encode_node_id(typename, key_parts.clone()).0 != node_id {
        return None;
    }
    Some(key_parts)
}

#[cfg(test)]
mod node_id_tests {
    use super::{decode_node_id, encode_node_id};
    use serde_json::{Value, json};

    /// Shared with graphql-orm-client and graphql-orm-router; never change.
    fn vectors() -> Vec<(&'static str, Vec<Value>, &'static str)> {
        vec![
            (
                "FameDevice",
                vec![json!("9b2c0f2e-1d1a-4f6e-9c1e-0a1b2c3d4e5f")],
                "WyJGYW1lRGV2aWNlIiwiOWIyYzBmMmUtMWQxYS00ZjZlLTljMWUtMGExYjJjM2Q0ZTVmIl0",
            ),
            (
                "FameEnrollmentTenantRecord",
                vec![json!("acme:sub.team_1")],
                "WyJGYW1lRW5yb2xsbWVudFRlbmFudFJlY29yZCIsImFjbWU6c3ViLnRlYW1fMSJd",
            ),
            (
                "ManagedSlot",
                vec![json!("north"), json!("a")],
                "WyJNYW5hZ2VkU2xvdCIsIm5vcnRoIiwiYSJd",
            ),
            ("Ticket", vec![json!(42)], "WyJUaWNrZXQiLDQyXQ"),
            (
                "Note",
                vec![json!("ünïcødé ✓")],
                "WyJOb3RlIiwiw7xuw69jw7hkw6kg4pyTIl0",
            ),
            (
                "Quote",
                vec![json!("say \"hi\"\\n")],
                "WyJRdW90ZSIsInNheSBcImhpXCJcXG4iXQ",
            ),
        ]
    }

    #[test]
    fn shared_vectors_encode_and_decode_exactly() {
        for (typename, parts, node_id) in vectors() {
            assert_eq!(encode_node_id(typename, parts.clone()).0, node_id);
            assert_eq!(
                decode_node_id(node_id, typename, parts.len()),
                Some(parts),
                "{typename}"
            );
        }
    }

    #[test]
    fn malformed_mismatched_and_non_canonical_ids_decode_to_none() {
        let ticket = "WyJUaWNrZXQiLDQyXQ";
        assert_eq!(decode_node_id(ticket, "Note", 1), None, "typename mismatch");
        assert_eq!(decode_node_id(ticket, "Ticket", 2), None, "part count");
        assert_eq!(
            decode_node_id("WyJUaWNrZXQiLDQyXQ==", "Ticket", 1),
            None,
            "padding"
        );
        assert_eq!(decode_node_id("not base64!", "Ticket", 1), None);
        assert_eq!(decode_node_id("", "Ticket", 1), None);
        let encode = |json: &str| {
            use base64::Engine as _;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
        };
        assert_eq!(
            decode_node_id(&encode("{}"), "Ticket", 1),
            None,
            "not an array"
        );
        assert_eq!(
            decode_node_id(&encode("[\"Ticket\", 42]"), "Ticket", 1),
            None,
            "whitespace"
        );
        assert_eq!(
            decode_node_id(&encode("[\"Ticket\",4.2e1]"), "Ticket", 1),
            None,
            "number spelling"
        );
        assert_eq!(
            decode_node_id(&encode("[\"Note\",\"\\u00fc\"]"), "Note", 1),
            None,
            "escaped non-ASCII"
        );
        assert_eq!(
            decode_node_id(&encode("[42,\"Ticket\"]"), "Ticket", 1),
            None
        );
    }
}
