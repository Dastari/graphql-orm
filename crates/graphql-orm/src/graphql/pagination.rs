#[derive(async_graphql::SimpleObject, Clone, Debug, Default)]
#[cfg_attr(feature = "field-case-pascal", graphql(rename_fields = "PascalCase"))]
#[cfg_attr(feature = "field-case-snake", graphql(rename_fields = "snake_case"))]
#[cfg_attr(
    feature = "field-case-screaming-snake",
    graphql(rename_fields = "SCREAMING_SNAKE_CASE")
)]
#[cfg_attr(feature = "field-case-lower", graphql(rename_fields = "lowercase"))]
#[cfg_attr(feature = "field-case-upper", graphql(rename_fields = "UPPERCASE"))]
pub struct PageInfo {
    #[cfg_attr(feature = "field-case-lower", graphql(name = "hasnextpage"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "HASNEXTPAGE"))]
    pub has_next_page: bool,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "haspreviouspage"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "HASPREVIOUSPAGE"))]
    pub has_previous_page: bool,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "startcursor"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "STARTCURSOR"))]
    pub start_cursor: Option<String>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "endcursor"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "ENDCURSOR"))]
    pub end_cursor: Option<String>,
    #[cfg_attr(feature = "field-case-lower", graphql(name = "totalcount"))]
    #[cfg_attr(feature = "field-case-upper", graphql(name = "TOTALCOUNT"))]
    pub total_count: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct Edge<T> {
    pub node: T,
    pub cursor: String,
}

#[derive(Clone, Debug)]
pub struct Connection<T> {
    pub edges: Vec<Edge<T>>,
    pub page_info: PageInfo,
}

pub fn encode_cursor(offset: i64) -> String {
    offset.to_string()
}

/// Portable scalar stored in a keyset cursor.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t", content = "v")]
pub enum KeysetValue {
    Null,
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Uuid(String),
    Bytes(Vec<u8>),
}

#[derive(serde::Serialize, serde::Deserialize)]
struct KeysetCursorEnvelope {
    version: u8,
    order: String,
    values: Vec<KeysetValue>,
    checksum: String,
}

fn cursor_checksum(version: u8, order: &str, values: &[KeysetValue]) -> String {
    let payload = serde_json::to_vec(&(version, order, values)).unwrap_or_default();
    let mut hash = 0xcbf29ce484222325u64;
    for byte in payload {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

/// Encode a versioned opaque cursor bound to one deterministic order.
pub fn encode_keyset_cursor(order: &str, values: Vec<KeysetValue>) -> String {
    let envelope = KeysetCursorEnvelope {
        version: 1,
        checksum: cursor_checksum(1, order, &values),
        order: order.to_string(),
        values,
    };
    let json = serde_json::to_vec(&envelope).expect("keyset cursor serialization is infallible");
    format!("gomk1.{}", hex_encode(&json))
}

/// Strictly decode a keyset cursor and reject legacy offsets, tampering,
/// unknown versions, order changes, and malformed scalar payloads.
pub fn decode_keyset_cursor(
    cursor: &str,
    expected_order: &str,
    expected_values: usize,
) -> Result<Vec<KeysetValue>, crate::graphql::errors::OrmPublicError> {
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
    let payload = cursor
        .strip_prefix("gomk1.")
        .and_then(hex_decode)
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::CursorInvalid))?;
    let envelope: KeysetCursorEnvelope = serde_json::from_slice(&payload)
        .map_err(|_| OrmPublicError::new(OrmErrorCode::CursorInvalid))?;
    if envelope.version != 1
        || envelope.order != expected_order
        || envelope.values.len() != expected_values
        || envelope.checksum != cursor_checksum(envelope.version, &envelope.order, &envelope.values)
    {
        return Err(OrmPublicError::new(OrmErrorCode::CursorInvalid));
    }
    Ok(envelope.values)
}

/// Bounded forward keyset request. Legacy offset cursors are never decoded.
#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct KeysetPageInput {
    pub after: Option<String>,
    pub limit: Option<i64>,
    #[graphql(default)]
    pub include_total_count: bool,
}

/// Relay-style bounded bidirectional keyset request.
#[derive(async_graphql::InputObject, Clone, Debug, Default, PartialEq, Eq)]
pub struct KeysetConnectionInput {
    /// Read forward after this opaque cursor.
    pub after: Option<String>,
    /// Read backward before this opaque cursor.
    pub before: Option<String>,
    /// Forward page size.
    pub first: Option<i64>,
    /// Backward page size.
    pub last: Option<i64>,
    /// Opt-in count; disabled by default to avoid unbounded count work.
    #[graphql(default)]
    pub include_total_count: bool,
}

/// Validated query direction for a keyset connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeysetWindowDirection {
    /// Canonical forward read.
    Forward,
    /// Reverse database read whose edges are restored to canonical order.
    Backward,
}

/// Parsed bounded keyset request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedKeysetConnection {
    /// Direction.
    pub direction: KeysetWindowDirection,
    /// Exclusive cursor in the selected direction.
    pub cursor: Option<String>,
    /// Bounded requested edge count.
    pub limit: i64,
    /// Whether total count was explicitly requested.
    pub include_total_count: bool,
}

impl KeysetConnectionInput {
    /// Validates direction/cursor combinations and clamps the requested page to
    /// the supplied maximum.
    ///
    /// An omitted `first`/`last` becomes a forward page using `default_limit`.
    /// `after` may use the default forward limit, while `before` requires an
    /// explicit `last`. Mixed directions and dual cursors fail closed.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error for ambiguous directions, a non-positive
    /// requested page, or `before` without `last`. Returns an authorization
    /// configuration error when either supplied limit bound is non-positive.
    pub fn validate(
        &self,
        default_limit: i64,
        maximum_limit: i64,
    ) -> Result<ValidatedKeysetConnection, crate::graphql::errors::OrmPublicError> {
        use crate::graphql::errors::{OrmErrorCode, OrmPublicError};

        if default_limit <= 0 || maximum_limit <= 0 {
            return Err(OrmPublicError::new(
                OrmErrorCode::AuthorizationMisconfigured,
            ));
        }
        if self.first.is_some() && self.last.is_some()
            || self.after.is_some() && self.before.is_some()
            || self.last.is_some() && self.after.is_some()
            || self.first.is_some() && self.before.is_some()
            || self.before.is_some() && self.last.is_none()
        {
            return Err(OrmPublicError::new(OrmErrorCode::InvalidInput));
        }

        let (direction, cursor, requested) = if let Some(last) = self.last {
            (KeysetWindowDirection::Backward, self.before.clone(), last)
        } else {
            (
                KeysetWindowDirection::Forward,
                self.after.clone(),
                self.first.unwrap_or(default_limit),
            )
        };
        if requested <= 0 {
            return Err(OrmPublicError::new(OrmErrorCode::InvalidInput));
        }

        Ok(ValidatedKeysetConnection {
            direction,
            cursor,
            limit: requested.min(maximum_limit),
            include_total_count: self.include_total_count,
        })
    }
}
