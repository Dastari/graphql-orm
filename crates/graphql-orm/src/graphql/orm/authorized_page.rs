//! Context-bound continuation cursors for authorized pages in handwritten
//! resolvers and query builders.
//!
//! [`EntityQuery::fetch_authorized_page`](super::EntityQuery::fetch_authorized_page)
//! runs the same bounded, re-authorized scan as `fetch_authorized_scan`. Its
//! continuation is sealed by this module with a service-owned key and is
//! accepted only under the server-authored [`AuthorizedPageBinding`] it was
//! issued under.
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
use crate::graphql::pagination::{KeysetValue, decode_keyset_cursor, encode_keyset_cursor};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    XChaCha20Poly1305,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use std::time::Duration;

/// Largest accepted [`AuthorizedPageBinding`], in bytes.
pub const AUTHORIZED_PAGE_MAX_BINDING_BYTES: usize = 4096;
/// Most components one [`AuthorizedPageBindingBuilder`] accepts.
pub const AUTHORIZED_PAGE_MAX_BINDING_COMPONENTS: usize = 32;
/// Most keys (the active key plus accept-only keys) in [`AuthorizedPageKeys`].
pub const AUTHORIZED_PAGE_MAX_KEYS: usize = 8;
/// Longest key identifier, in bytes.
pub const AUTHORIZED_PAGE_MAX_KEY_ID_BYTES: usize = 32;
/// Default cursor size bound, in bytes.
pub const AUTHORIZED_PAGE_DEFAULT_MAX_CURSOR_BYTES: usize = 8 * 1024;
/// Largest configurable cursor size bound, in bytes.
pub const AUTHORIZED_PAGE_CURSOR_BYTES_CEILING: usize = 64 * 1024;
/// Default page size bound, and the page size used when a request names none.
pub const AUTHORIZED_PAGE_DEFAULT_MAX_PAGE_SIZE: u32 = 100;
/// Default cursor lifetime.
pub const AUTHORIZED_PAGE_DEFAULT_CURSOR_TTL: Duration = Duration::from_secs(60 * 60);
/// Longest configurable cursor lifetime.
pub const AUTHORIZED_PAGE_CURSOR_TTL_CEILING: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// `reason` extension of the `CURSOR_INVALID` error for a cursor that
/// authenticated under the current binding and is past its lifetime.
pub const AUTHORIZED_PAGE_CURSOR_EXPIRED_REASON: &str = "AUTHORIZED_PAGE_CURSOR_EXPIRED";
/// `reason` extension of the `CURSOR_INVALID` error for a cursor that names a
/// key identifier this service no longer accepts (or never did).
pub const AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE_REASON: &str =
    "AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE";

/// Codec version 1 token prefix. A later codec gets a new prefix; a token
/// with any other prefix is refused, never reinterpreted.
const TOKEN_PREFIX: &str = "gomp1";
/// Domain separator authenticated with every version 1 token.
const AAD_DOMAIN: &[u8] = b"graphql-orm/authorized-page/v1";
const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;
const EXPIRY_BYTES: usize = 8;

/// Why a context binding was refused. Carries no part of the binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthorizedPageBindingError {
    /// The binding, a component name or a component value is empty.
    Empty,
    /// The binding is longer than [`AUTHORIZED_PAGE_MAX_BINDING_BYTES`] or has
    /// more than [`AUTHORIZED_PAGE_MAX_BINDING_COMPONENTS`] components.
    TooLarge,
    /// A component name was used twice.
    DuplicateComponent,
}
impl std::fmt::Display for AuthorizedPageBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Empty => "authorized page binding has an empty part",
            Self::TooLarge => "authorized page binding is too large",
            Self::DuplicateComponent => "authorized page binding repeats a component",
        })
    }
}
impl std::error::Error for AuthorizedPageBindingError {}
impl AuthorizedPageBindingError {
    /// The sanitized public error for a resolver to return. A refused binding
    /// is a server defect, never client input, so this is
    /// `AUTHORIZATION_MISCONFIGURED` with its default message.
    pub fn into_graphql_error(self) -> async_graphql::Error {
        OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured).into_graphql_error()
    }
}

/// The server's statement of the context a page sequence belongs to.
///
/// The resolver builds it on **every** request from trusted server state only:
/// for example the committed snapshot generation, the source registration and
/// its incarnation, the workspace, the original and current principal and
/// their grants, and the credential, configuration and placement revisions.
/// Never build it from a GraphQL argument, a header the client controls, or
/// any other browser value. Leave nothing out that must end a page sequence
/// when it changes: the ORM cannot tell that a binding is incomplete.
///
/// The bytes are opaque to the ORM. They are authenticated with each
/// continuation and are never placed in it, so a client cannot read them, and
/// a continuation issued under different bytes is refused before any row is
/// read. `Debug` output is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizedPageBinding(Vec<u8>);
impl AuthorizedPageBinding {
    /// Use bytes the server already has in a canonical form, such as a digest
    /// of its context stamps. One to [`AUTHORIZED_PAGE_MAX_BINDING_BYTES`] bytes.
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self, AuthorizedPageBindingError> {
        let bytes = bytes.as_ref();
        if bytes.is_empty() {
            return Err(AuthorizedPageBindingError::Empty);
        }
        if bytes.len() > AUTHORIZED_PAGE_MAX_BINDING_BYTES {
            return Err(AuthorizedPageBindingError::TooLarge);
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Compose a binding from named components without concatenation
    /// ambiguity.
    pub fn builder() -> AuthorizedPageBindingBuilder {
        AuthorizedPageBindingBuilder {
            names: Vec::new(),
            bytes: Vec::new(),
            error: None,
        }
    }
}
impl std::fmt::Debug for AuthorizedPageBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthorizedPageBinding([redacted])")
    }
}

/// Length-delimited composition of named binding components.
///
/// Components are bound in the order they are added, with their names, so the
/// resolver must add the same components in the same order on every request.
/// Empty names, empty values and repeated names are refused by
/// [`build`](Self::build): state an absent value explicitly (for example
/// `"none"`) rather than leaving it out.
#[derive(Clone)]
pub struct AuthorizedPageBindingBuilder {
    names: Vec<Vec<u8>>,
    bytes: Vec<u8>,
    error: Option<AuthorizedPageBindingError>,
}
impl AuthorizedPageBindingBuilder {
    /// Add one named component.
    pub fn component(mut self, name: &str, value: impl AsRef<[u8]>) -> Self {
        if self.error.is_some() {
            return self;
        }
        let value = value.as_ref();
        let name = name.as_bytes();
        self.error = if name.is_empty() || value.is_empty() {
            Some(AuthorizedPageBindingError::Empty)
        } else if self.names.iter().any(|known| known == name) {
            Some(AuthorizedPageBindingError::DuplicateComponent)
        } else if self.names.len() == AUTHORIZED_PAGE_MAX_BINDING_COMPONENTS
            || self.bytes.len() + 8 + name.len() + value.len() > AUTHORIZED_PAGE_MAX_BINDING_BYTES
        {
            Some(AuthorizedPageBindingError::TooLarge)
        } else {
            None
        };
        if self.error.is_none() {
            push_part(&mut self.bytes, name);
            push_part(&mut self.bytes, value);
            self.names.push(name.to_vec());
        }
        self
    }

    /// Finish the binding, or report the first refused component.
    pub fn build(self) -> Result<AuthorizedPageBinding, AuthorizedPageBindingError> {
        match self.error {
            Some(error) => Err(error),
            None => AuthorizedPageBinding::from_bytes(self.bytes),
        }
    }
}
impl std::fmt::Debug for AuthorizedPageBindingBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthorizedPageBindingBuilder([redacted])")
    }
}

/// Bounded parts only: every caller has already capped `part` far below `u32::MAX`.
fn push_part(bytes: &mut Vec<u8>, part: &[u8]) {
    bytes.extend_from_slice(&(part.len() as u32).to_be_bytes());
    bytes.extend_from_slice(part);
}

/// Why a key ring or page configuration was refused. Carries no key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthorizedPageConfigError {
    /// A key identifier is empty, longer than
    /// [`AUTHORIZED_PAGE_MAX_KEY_ID_BYTES`], or has a character other than
    /// ASCII letters, digits, `_` and `-`.
    InvalidKeyId,
    /// A key identifier was used twice.
    DuplicateKeyId,
    /// More than [`AUTHORIZED_PAGE_MAX_KEYS`] keys.
    TooManyKeys,
    /// A bound is zero, inconsistent or above its ceiling.
    InvalidBound,
}
impl std::fmt::Display for AuthorizedPageConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidKeyId => "invalid authorized page key identifier",
            Self::DuplicateKeyId => "duplicate authorized page key identifier",
            Self::TooManyKeys => "too many authorized page keys",
            Self::InvalidBound => "invalid authorized page bound",
        })
    }
}
impl std::error::Error for AuthorizedPageConfigError {}

fn valid_key_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= AUTHORIZED_PAGE_MAX_KEY_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The service-owned keys that seal and open page continuations.
///
/// Each key is a secret, randomly generated 32 bytes owned by the service and
/// held in its encrypted settings (or equivalent root-secret store), from
/// which the host loads it at start-up. Never derive a key from a provider
/// credential, a user or tenant secret, or any value a client can see or
/// choose, and never send one to a client. Every replica that serves the same
/// pages needs the same keys under the same identifiers.
///
/// A key identifier is a public, non-secret label that appears in each cursor.
///
/// # Rotation
///
/// 1. Add the new key with [`accepting`](Self::accepting) on every replica.
/// 2. Make it the active key (first argument of [`new`](Self::new)) and keep
///    the previous key with `accepting` for at least the cursor lifetime.
/// 3. Remove the previous key. Cursors sealed with it are then refused with
///    [`AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE_REASON`].
///
/// `Debug` output shows no key material.
#[derive(Clone)]
pub struct AuthorizedPageKeys {
    /// The active (sealing) key is first.
    keys: Vec<(String, [u8; 32])>,
}
impl AuthorizedPageKeys {
    /// A key ring whose only key seals new cursors and opens its own.
    pub fn new(
        active_key_id: impl Into<String>,
        active_key: [u8; 32],
    ) -> Result<Self, AuthorizedPageConfigError> {
        let id = active_key_id.into();
        if !valid_key_id(&id) {
            return Err(AuthorizedPageConfigError::InvalidKeyId);
        }
        Ok(Self {
            keys: vec![(id, active_key)],
        })
    }

    /// Also open cursors sealed with this key, without sealing new ones.
    pub fn accepting(
        mut self,
        key_id: impl Into<String>,
        key: [u8; 32],
    ) -> Result<Self, AuthorizedPageConfigError> {
        let id = key_id.into();
        if !valid_key_id(&id) {
            return Err(AuthorizedPageConfigError::InvalidKeyId);
        }
        if self.keys.iter().any(|(known, _)| *known == id) {
            return Err(AuthorizedPageConfigError::DuplicateKeyId);
        }
        if self.keys.len() == AUTHORIZED_PAGE_MAX_KEYS {
            return Err(AuthorizedPageConfigError::TooManyKeys);
        }
        self.keys.push((id, key));
        Ok(self)
    }
}
impl std::fmt::Debug for AuthorizedPageKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizedPageKeys")
            .field("keys", &self.keys.len())
            .finish_non_exhaustive()
    }
}

/// Server-owned keys and bounds for context-bound authorized pages.
///
/// `EntityQuery::fetch_authorized_page` fails closed until this is installed
/// with `Database::with_authorized_page_config`. It is independent of
/// `AuthorizedScanConfig`: installing one does not enable the other API.
#[derive(Clone, Debug)]
pub struct AuthorizedPageConfig {
    keys: AuthorizedPageKeys,
    batch_size: u32,
    max_scanned_rows: u32,
    max_page_size: u32,
    max_cursor_bytes: usize,
    cursor_ttl: Duration,
}
impl AuthorizedPageConfig {
    /// `batch_size` rows are fetched per statement and at most
    /// `max_scanned_rows` candidates are examined per request. Both must be
    /// non-zero and `batch_size` cannot exceed `max_scanned_rows`.
    pub fn new(
        keys: AuthorizedPageKeys,
        batch_size: u32,
        max_scanned_rows: u32,
    ) -> Result<Self, AuthorizedPageConfigError> {
        if batch_size == 0 || batch_size > max_scanned_rows {
            return Err(AuthorizedPageConfigError::InvalidBound);
        }
        Ok(Self {
            keys,
            batch_size,
            max_scanned_rows,
            max_page_size: AUTHORIZED_PAGE_DEFAULT_MAX_PAGE_SIZE,
            max_cursor_bytes: AUTHORIZED_PAGE_DEFAULT_MAX_CURSOR_BYTES,
            cursor_ttl: AUTHORIZED_PAGE_DEFAULT_CURSOR_TTL,
        })
    }

    /// Largest page a request may ask for; a larger `limit` is refused with
    /// `PAGE_LIMIT_EXCEEDED`, never truncated. A request without a limit gets
    /// the smaller of this and [`AUTHORIZED_PAGE_DEFAULT_MAX_PAGE_SIZE`].
    pub fn with_max_page_size(mut self, rows: u32) -> Result<Self, AuthorizedPageConfigError> {
        if rows == 0 {
            return Err(AuthorizedPageConfigError::InvalidBound);
        }
        self.max_page_size = rows;
        Ok(self)
    }

    /// Longest cursor accepted or issued, in bytes: 256 to
    /// [`AUTHORIZED_PAGE_CURSOR_BYTES_CEILING`].
    pub fn with_max_cursor_bytes(
        mut self,
        bytes: usize,
    ) -> Result<Self, AuthorizedPageConfigError> {
        if !(256..=AUTHORIZED_PAGE_CURSOR_BYTES_CEILING).contains(&bytes) {
            return Err(AuthorizedPageConfigError::InvalidBound);
        }
        self.max_cursor_bytes = bytes;
        Ok(self)
    }

    /// How long a continuation stays usable after the page that issued it.
    /// Each page issues a fresh continuation, so this bounds the pause between
    /// two requests, not the whole traversal. Non-zero, at most
    /// [`AUTHORIZED_PAGE_CURSOR_TTL_CEILING`]. Replicas compare it with their
    /// own clocks.
    pub fn with_cursor_ttl(mut self, ttl: Duration) -> Result<Self, AuthorizedPageConfigError> {
        if ttl.is_zero() || ttl > AUTHORIZED_PAGE_CURSOR_TTL_CEILING {
            return Err(AuthorizedPageConfigError::InvalidBound);
        }
        self.cursor_ttl = ttl;
        Ok(self)
    }

    /// Rows fetched per statement.
    pub fn batch_size(&self) -> u32 {
        self.batch_size
    }
    /// Candidates examined per request.
    pub fn max_scanned_rows(&self) -> u32 {
        self.max_scanned_rows
    }
    /// Largest page a request may ask for.
    pub fn max_page_size(&self) -> u32 {
        self.max_page_size
    }
    /// Longest cursor accepted or issued, in bytes.
    pub fn max_cursor_bytes(&self) -> usize {
        self.max_cursor_bytes
    }
    /// Lifetime of each continuation.
    pub fn cursor_ttl(&self) -> Duration {
        self.cursor_ttl
    }

    /// The page size for a request, or the public refusal.
    pub(super) fn resolve_limit(&self, limit: Option<i64>) -> async_graphql::Result<usize> {
        match limit {
            None => Ok(self
                .max_page_size
                .min(AUTHORIZED_PAGE_DEFAULT_MAX_PAGE_SIZE) as usize),
            Some(limit) if limit <= 0 => {
                Err(OrmPublicError::new(OrmErrorCode::InvalidInput).into_graphql_error())
            }
            Some(limit) if limit > i64::from(self.max_page_size) => {
                Err(OrmPublicError::new(OrmErrorCode::PageLimitExceeded).into_graphql_error())
            }
            Some(limit) => Ok(limit as usize),
        }
    }

    pub(super) fn seal(
        &self,
        binding: &AuthorizedPageBinding,
        entity: &str,
        fingerprint: &str,
        values: Vec<KeysetValue>,
    ) -> async_graphql::Result<String> {
        self.seal_at(binding, entity, fingerprint, values, now_ms())
    }

    pub(super) fn open(
        &self,
        binding: &AuthorizedPageBinding,
        entity: &str,
        fingerprint: &str,
        expected_values: usize,
        cursor: &str,
    ) -> async_graphql::Result<Vec<KeysetValue>> {
        self.open_at(
            binding,
            entity,
            fingerprint,
            expected_values,
            cursor,
            now_ms(),
        )
    }

    fn seal_at(
        &self,
        binding: &AuthorizedPageBinding,
        entity: &str,
        fingerprint: &str,
        values: Vec<KeysetValue>,
        now_ms: u64,
    ) -> async_graphql::Result<String> {
        // A position that cannot be sealed within the bounds is a server-side
        // limit, reported without the position.
        let unavailable =
            || OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured).into_graphql_error();
        let (key_id, key) = &self.keys.keys[0];
        let ttl_ms = u64::try_from(self.cursor_ttl.as_millis()).map_err(|_| unavailable())?;
        let expires_at = now_ms.checked_add(ttl_ms).ok_or_else(unavailable)?;
        let mut plaintext = expires_at.to_be_bytes().to_vec();
        plaintext.extend_from_slice(encode_keyset_cursor(fingerprint, values).as_bytes());
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = XChaCha20Poly1305::new(key.into())
            .encrypt(
                &nonce,
                Payload {
                    msg: &plaintext,
                    aad: &associated_data(key_id, entity, binding),
                },
            )
            .map_err(|_| unavailable())?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&ciphertext);
        let token = format!("{TOKEN_PREFIX}.{key_id}.{}", URL_SAFE_NO_PAD.encode(sealed));
        if token.len() > self.max_cursor_bytes {
            return Err(unavailable());
        }
        Ok(token)
    }

    fn open_at(
        &self,
        binding: &AuthorizedPageBinding,
        entity: &str,
        fingerprint: &str,
        expected_values: usize,
        cursor: &str,
        now_ms: u64,
    ) -> async_graphql::Result<Vec<KeysetValue>> {
        let invalid = || OrmPublicError::new(OrmErrorCode::CursorInvalid).into_graphql_error();
        // Size is checked before any parsing or decoding work.
        if cursor.len() > self.max_cursor_bytes {
            return Err(invalid());
        }
        let mut pieces = cursor.split('.');
        let (Some(TOKEN_PREFIX), Some(key_id), Some(encoded), None) =
            (pieces.next(), pieces.next(), pieces.next(), pieces.next())
        else {
            return Err(invalid());
        };
        if !valid_key_id(key_id) {
            return Err(invalid());
        }
        let sealed = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
        if sealed.len() < NONCE_BYTES + TAG_BYTES + EXPIRY_BYTES {
            return Err(invalid());
        }
        let Some((_, key)) = self.keys.keys.iter().find(|(known, _)| known == key_id) else {
            return Err(invalid_with_reason(
                AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE_REASON,
            ));
        };
        let (nonce, ciphertext) = sealed.split_at(NONCE_BYTES);
        // Tampering, truncation, a wrong key and a changed binding or entity
        // all fail here, indistinguishably, before the position is read.
        let plaintext = XChaCha20Poly1305::new(key.into())
            .decrypt(
                nonce.into(),
                Payload {
                    msg: ciphertext,
                    aad: &associated_data(key_id, entity, binding),
                },
            )
            .map_err(|_| invalid())?;
        let (expiry, position) = plaintext
            .split_at_checked(EXPIRY_BYTES)
            .ok_or_else(invalid)?;
        let expires_at = u64::from_be_bytes(expiry.try_into().map_err(|_| invalid())?);
        if now_ms >= expires_at {
            return Err(invalid_with_reason(AUTHORIZED_PAGE_CURSOR_EXPIRED_REASON));
        }
        let position = std::str::from_utf8(position).map_err(|_| invalid())?;
        decode_keyset_cursor(position, fingerprint, expected_values)
            .map_err(|error| error.into_graphql_error())
    }
}

fn invalid_with_reason(reason: &'static str) -> async_graphql::Error {
    use async_graphql::ErrorExtensions;
    OrmPublicError::new(OrmErrorCode::CursorInvalid)
        .into_graphql_error()
        .extend_with(|_, extensions| extensions.set("reason", reason))
}

fn associated_data(key_id: &str, entity: &str, binding: &AuthorizedPageBinding) -> Vec<u8> {
    let mut bytes =
        Vec::with_capacity(AAD_DOMAIN.len() + key_id.len() + entity.len() + binding.0.len() + 16);
    for part in [
        AAD_DOMAIN,
        key_id.as_bytes(),
        entity.as_bytes(),
        binding.0.as_slice(),
    ] {
        push_part(&mut bytes, part);
    }
    bytes
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        // A clock before the epoch cannot establish that a cursor is live.
        .unwrap_or(u64::MAX)
}

/// One request for a context-bound authorized page.
///
/// `binding` comes from the server on every request. `after` and `limit` may
/// come from the client: `after` is the previous page's
/// `page_info.continuation`, verbatim.
#[derive(Clone, Debug)]
pub struct AuthorizedPageRequest {
    pub(super) binding: AuthorizedPageBinding,
    pub(super) after: Option<String>,
    pub(super) limit: Option<i64>,
}
impl AuthorizedPageRequest {
    /// The first page under `binding`, with the default page size.
    pub fn new(binding: AuthorizedPageBinding) -> Self {
        Self {
            binding,
            after: None,
            limit: None,
        }
    }
    /// Continue after a continuation issued under the same binding.
    pub fn after(mut self, continuation: Option<String>) -> Self {
        self.after = continuation;
        self
    }
    /// Ask for at most this many visible rows.
    pub fn limit(mut self, limit: Option<i64>) -> Self {
        self.limit = limit;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTITY: &str = "Record";
    const ORDER: &str = "page:Record:rank asc, id asc";

    fn config(id: &str, key: u8) -> AuthorizedPageConfig {
        AuthorizedPageConfig::new(AuthorizedPageKeys::new(id, [key; 32]).unwrap(), 4, 16).unwrap()
    }
    fn binding(generation: &str) -> AuthorizedPageBinding {
        AuthorizedPageBinding::builder()
            .component("snapshot", generation)
            .component("workspace", "w1")
            .build()
            .unwrap()
    }
    fn position() -> Vec<KeysetValue> {
        vec![KeysetValue::Int(7), KeysetValue::String("secret-id".into())]
    }
    fn extension(error: &async_graphql::Error, name: &str) -> Option<String> {
        match error.extensions.as_ref()?.get(name)? {
            async_graphql::Value::String(value) => Some(value.clone()),
            _ => None,
        }
    }
    fn refused(result: async_graphql::Result<Vec<KeysetValue>>) -> Option<String> {
        let error = result.unwrap_err();
        assert_eq!(extension(&error, "code").as_deref(), Some("CURSOR_INVALID"));
        assert_eq!(error.message, "invalid cursor");
        extension(&error, "reason")
    }

    #[test]
    fn round_trips_and_is_bound_to_binding_entity_and_order() {
        let config = config("k1", 1);
        let token = config
            .seal_at(&binding("g1"), ENTITY, ORDER, position(), 1_000)
            .unwrap();
        let open = |binding: &AuthorizedPageBinding, entity, order| {
            config.open_at(binding, entity, order, 2, &token, 2_000)
        };
        assert_eq!(open(&binding("g1"), ENTITY, ORDER).unwrap(), position());
        assert_eq!(refused(open(&binding("g2"), ENTITY, ORDER)), None);
        assert_eq!(refused(open(&binding("g1"), "Other", ORDER)), None);
        assert_eq!(refused(open(&binding("g1"), ENTITY, "page:Record:x")), None);
        // Neither the position nor the binding is readable in the token.
        let body = URL_SAFE_NO_PAD
            .decode(token.rsplit('.').next().unwrap())
            .unwrap();
        for needle in [b"secret-id".as_slice(), b"snapshot", b"g1", b"w1"] {
            assert!(!body.windows(needle.len()).any(|window| window == needle));
            assert!(!token.contains(std::str::from_utf8(needle).unwrap()));
        }
        // Each token is sealed under a fresh nonce.
        let again = config
            .seal_at(&binding("g1"), ENTITY, ORDER, position(), 1_000)
            .unwrap();
        assert_ne!(token, again);
    }

    #[test]
    fn expiry_is_exact_and_reported_only_after_authentication() {
        let config = config("k1", 1)
            .with_cursor_ttl(Duration::from_millis(500))
            .unwrap();
        let token = config
            .seal_at(&binding("g1"), ENTITY, ORDER, position(), 1_000)
            .unwrap();
        let open = |binding: &AuthorizedPageBinding, now| {
            config.open_at(binding, ENTITY, ORDER, 2, &token, now)
        };
        assert!(open(&binding("g1"), 1_499).is_ok());
        for now in [1_500, 9_999, u64::MAX] {
            assert_eq!(
                refused(open(&binding("g1"), now)).as_deref(),
                Some(AUTHORIZED_PAGE_CURSOR_EXPIRED_REASON)
            );
        }
        // A wrong binding does not learn that the cursor has expired.
        assert_eq!(refused(open(&binding("g2"), 9_999)), None);
    }

    #[test]
    fn rotation_accepts_retained_keys_and_refuses_removed_and_wrong_keys() {
        let old = config("k1", 1);
        let token = old
            .seal_at(&binding("g1"), ENTITY, ORDER, position(), 0)
            .unwrap();
        let rotated = AuthorizedPageConfig::new(
            AuthorizedPageKeys::new("k2", [2; 32])
                .unwrap()
                .accepting("k1", [1; 32])
                .unwrap(),
            4,
            16,
        )
        .unwrap();
        let open = |config: &AuthorizedPageConfig, token: &str| {
            config.open_at(&binding("g1"), ENTITY, ORDER, 2, token, 1)
        };
        assert_eq!(open(&rotated, &token).unwrap(), position());
        let fresh = rotated
            .seal_at(&binding("g1"), ENTITY, ORDER, position(), 0)
            .unwrap();
        assert!(fresh.starts_with("gomp1.k2."));
        // A replica that has not yet learned the new key refuses its cursors.
        assert_eq!(
            refused(open(&old, &fresh)).as_deref(),
            Some(AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE_REASON)
        );
        assert_eq!(
            refused(open(&config("k2", 2), &token)).as_deref(),
            Some(AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE_REASON)
        );
        // The same identifier over different key bytes fails authentication.
        assert_eq!(refused(open(&config("k1", 9), &token)), None);
        // Relabelling a token does not move it to another key.
        let relabelled = token.replacen("gomp1.k1.", "gomp1.k2.", 1);
        assert_eq!(refused(open(&rotated, &relabelled)), None);
    }

    #[test]
    fn malformed_tampered_truncated_and_oversized_tokens_are_refused() {
        let config = config("k1", 1);
        let token = config
            .seal_at(&binding("g1"), ENTITY, ORDER, position(), 0)
            .unwrap();
        let open = |token: &str| config.open_at(&binding("g1"), ENTITY, ORDER, 2, token, 1);
        assert!(open(&token).is_ok());
        let body_start = token.rfind('.').unwrap() + 1;
        // Every single-character change to the sealed body is refused.
        for index in body_start..token.len() {
            let mut bytes = token.clone().into_bytes();
            bytes[index] = if bytes[index] == b'A' { b'B' } else { b'A' };
            assert_eq!(refused(open(std::str::from_utf8(&bytes).unwrap())), None);
        }
        for length in 0..token.len() {
            assert_eq!(refused(open(&token[..length])), None);
        }
        for malformed in [
            String::new(),
            "gomp1".into(),
            "gomp1..".into(),
            format!("gomp2{}", &token[5..]),
            format!("goms1{}", &token[5..]),
            format!("{token}.x"),
            format!("{token}="),
            token.replacen("gomp1.k1.", "gomp1.k 1.", 1),
            format!("gomp1.{}.{}", "k".repeat(33), &token[body_start..]),
            format!(
                "{token}{}",
                "A".repeat(AUTHORIZED_PAGE_DEFAULT_MAX_CURSOR_BYTES)
            ),
        ] {
            assert_eq!(refused(open(&malformed)), None);
        }
        // A position that does not fit the bound is never issued.
        let tight = config.clone().with_max_cursor_bytes(256).unwrap();
        let large = vec![KeysetValue::String("x".repeat(512)), KeysetValue::Int(1)];
        assert!(
            tight
                .seal_at(&binding("g1"), ENTITY, ORDER, large, 0)
                .is_err()
        );
    }

    #[test]
    fn bindings_keys_and_bounds_are_validated_without_echoing_values() {
        use AuthorizedPageBindingError as B;
        use AuthorizedPageConfigError as C;
        assert_eq!(AuthorizedPageBinding::from_bytes([]).unwrap_err(), B::Empty);
        assert!(AuthorizedPageBinding::from_bytes([1; AUTHORIZED_PAGE_MAX_BINDING_BYTES]).is_ok());
        assert_eq!(
            AuthorizedPageBinding::from_bytes([1; AUTHORIZED_PAGE_MAX_BINDING_BYTES + 1])
                .unwrap_err(),
            B::TooLarge
        );
        let builder = AuthorizedPageBinding::builder;
        assert_eq!(builder().build().unwrap_err(), B::Empty);
        assert_eq!(builder().component("", "v").build().unwrap_err(), B::Empty);
        assert_eq!(builder().component("n", "").build().unwrap_err(), B::Empty);
        assert_eq!(
            builder()
                .component("n", "a")
                .component("n", "b")
                .build()
                .unwrap_err(),
            B::DuplicateComponent
        );
        assert_eq!(
            builder()
                .component("n", [0; AUTHORIZED_PAGE_MAX_BINDING_BYTES])
                .build()
                .unwrap_err(),
            B::TooLarge
        );
        let mut many = builder();
        for index in 0..=AUTHORIZED_PAGE_MAX_BINDING_COMPONENTS {
            many = many.component(&format!("c{index}"), "v");
        }
        assert_eq!(many.build().unwrap_err(), B::TooLarge);
        // Components cannot be shifted between names and values.
        let pair = |a: &str, b: &str| builder().component(a, b).build().unwrap();
        assert_ne!(pair("ab", "c"), pair("a", "bc"));
        assert_ne!(
            builder()
                .component("a", "1")
                .component("b", "2")
                .build()
                .unwrap(),
            builder()
                .component("b", "2")
                .component("a", "1")
                .build()
                .unwrap()
        );
        let secret = binding("generation-secret");
        assert!(!format!("{secret:?}").contains("generation-secret"));

        for id in ["", "has space", "dot.ted", &"k".repeat(33)] {
            assert_eq!(
                AuthorizedPageKeys::new(id, [1; 32]).unwrap_err(),
                C::InvalidKeyId
            );
        }
        let keys = AuthorizedPageKeys::new("k0", [0xAB; 32]).unwrap();
        assert_eq!(
            keys.clone().accepting("k0", [2; 32]).unwrap_err(),
            C::DuplicateKeyId
        );
        let mut full = keys.clone();
        for index in 1..AUTHORIZED_PAGE_MAX_KEYS {
            full = full
                .accepting(format!("k{index}"), [index as u8; 32])
                .unwrap();
        }
        assert_eq!(
            full.accepting("extra", [9; 32]).unwrap_err(),
            C::TooManyKeys
        );
        let new = |batch, budget| AuthorizedPageConfig::new(keys.clone(), batch, budget);
        assert_eq!(new(0, 4).unwrap_err(), C::InvalidBound);
        assert_eq!(new(5, 4).unwrap_err(), C::InvalidBound);
        let config = new(4, 4).unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("171") && !debug.to_lowercase().contains("ab, ab"));
        assert!(config.clone().with_max_page_size(0).is_err());
        assert!(config.clone().with_max_cursor_bytes(255).is_err());
        assert!(
            config
                .clone()
                .with_max_cursor_bytes(AUTHORIZED_PAGE_CURSOR_BYTES_CEILING + 1)
                .is_err()
        );
        assert!(config.clone().with_cursor_ttl(Duration::ZERO).is_err());
        assert!(
            config
                .clone()
                .with_cursor_ttl(AUTHORIZED_PAGE_CURSOR_TTL_CEILING + Duration::from_secs(1))
                .is_err()
        );
        let config = config.with_max_page_size(3).unwrap();
        assert_eq!(config.resolve_limit(None).unwrap(), 3);
        assert_eq!(config.resolve_limit(Some(3)).unwrap(), 3);
        let code = |limit| extension(&config.resolve_limit(Some(limit)).unwrap_err(), "code");
        assert_eq!(code(4).as_deref(), Some("PAGE_LIMIT_EXCEEDED"));
        assert_eq!(code(0).as_deref(), Some("INVALID_INPUT"));
        assert_eq!(code(-1).as_deref(), Some("INVALID_INPUT"));
    }
}
