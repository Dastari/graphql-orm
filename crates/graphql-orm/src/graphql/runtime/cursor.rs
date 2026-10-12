use super::RuntimeGraphqlError;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::future::BoxFuture;
use std::fmt;

/// Explicit cursor privacy profile. Protection is never auto-detected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeCursorProfile {
    /// The host must supply authenticated encryption of the complete envelope.
    #[default]
    AuthenticatedEncryption,
    /// Explicitly disclose the existing framework-neutral cursor envelope.
    Unprotected,
}

/// Bounds checked before decoding and after every host provider call.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeCursorProtectionLimits {
    pub max_plaintext_bytes: usize,
    pub max_key_id_bytes: usize,
    pub max_associated_data_bytes: usize,
    pub max_overhead_bytes: usize,
    pub max_token_bytes: usize,
}
impl Default for RuntimeCursorProtectionLimits {
    fn default() -> Self {
        Self {
            max_plaintext_bytes: 16 * 1024,
            max_key_id_bytes: 64,
            max_associated_data_bytes: 16 * 1024,
            max_overhead_bytes: 512,
            max_token_bytes: 32 * 1024,
        }
    }
}

/// Trusted host namespace; never taken from GraphQL arguments.
#[derive(Clone)]
pub struct RuntimeCursorAudience(String);
impl RuntimeCursorAudience {
    pub fn new(value: impl Into<String>) -> Result<Self, RuntimeGraphqlError> {
        let value = value.into();
        if value.is_empty() || value.len() > 1024 {
            return Err(RuntimeGraphqlError::new("invalid_composition"));
        }
        Ok(Self(value))
    }
}
impl fmt::Debug for RuntimeCursorAudience {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RuntimeCursorAudience([redacted])")
    }
}

/// Canonical associated data constructed only from trusted execution state.
///
/// Providers must authenticate these exact bytes. Debug never displays them.
#[derive(Clone)]
pub struct RuntimeCursorContext(Vec<u8>);
impl RuntimeCursorContext {
    /// Stable versioned associated data for the host's AEAD operation.
    pub fn associated_data(&self) -> &[u8] {
        &self.0
    }
    pub(super) fn new(
        audience: &RuntimeCursorAudience,
        scope: &str,
        parts: &[&[u8]],
        limits: RuntimeCursorProtectionLimits,
    ) -> Result<Self, RuntimeGraphqlError> {
        let mut bytes = Vec::new();
        // Length-delimited components prevent concatenation ambiguity.
        for part in [
            b"gormgqlc1".as_slice(),
            audience.0.as_bytes(),
            scope.as_bytes(),
        ]
        .into_iter()
        .chain(parts.iter().copied())
        {
            let length = u32::try_from(part.len()).map_err(|_| invalid())?;
            let required = bytes
                .len()
                .checked_add(4)
                .and_then(|n| n.checked_add(part.len()))
                .ok_or_else(invalid)?;
            if required > limits.max_associated_data_bytes {
                return Err(invalid());
            }
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes.extend_from_slice(part);
        }
        Ok(Self(bytes))
    }
}
impl fmt::Debug for RuntimeCursorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RuntimeCursorContext([redacted])")
    }
}

/// Public key label plus confidential provider nonce/tag/ciphertext framing.
#[derive(Clone)]
pub struct RuntimeSealedCursor {
    key_id: String,
    ciphertext: Vec<u8>,
}
impl RuntimeSealedCursor {
    pub fn new(
        key_id: impl Into<String>,
        ciphertext: Vec<u8>,
        limits: RuntimeCursorProtectionLimits,
    ) -> Result<Self, RuntimeCursorProtectionError> {
        let result = Self {
            key_id: key_id.into(),
            ciphertext,
        };
        result
            .validate(limits)
            .map_err(|_| RuntimeCursorProtectionError::InvalidCursor)?;
        Ok(result)
    }
    fn validate(&self, limits: RuntimeCursorProtectionLimits) -> Result<(), RuntimeGraphqlError> {
        validate_key(&self.key_id, limits)?;
        let max = limits
            .max_plaintext_bytes
            .checked_add(limits.max_overhead_bytes)
            .ok_or_else(invalid)?;
        if self.ciphertext.is_empty() || self.ciphertext.len() > max {
            return Err(invalid());
        }
        Ok(())
    }
}
impl fmt::Debug for RuntimeSealedCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RuntimeSealedCursor([redacted])")
    }
}

/// Provider errors deliberately cannot carry crypto or plaintext diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeCursorProtectionError {
    InvalidCursor,
    /// Unknown/retired key, expired token, or unavailable provider.
    CursorUnavailable,
}
impl fmt::Display for RuntimeCursorProtectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidCursor => "invalid_cursor",
            Self::CursorUnavailable => "cursor_unavailable",
        })
    }
}
impl std::error::Error for RuntimeCursorProtectionError {}
impl From<RuntimeCursorProtectionError> for RuntimeGraphqlError {
    fn from(value: RuntimeCursorProtectionError) -> Self {
        Self::new(match value {
            RuntimeCursorProtectionError::InvalidCursor => "invalid_cursor",
            RuntimeCursorProtectionError::CursorUnavailable => "cursor_unavailable",
        })
    }
}

/// Host-owned authenticated encryption, keys, nonces, rotation and expiry.
///
/// `seal` must encrypt the **entire** plaintext and authenticate the exact
/// context bytes. Signing or encoding readable data violates this contract.
/// `open` uses a bounded allowed key ring and authenticates before returning.
/// The host must honor all supplied bounds; no callback error permits fallback.
pub trait RuntimeCursorProtector: Send + Sync + 'static {
    fn seal<'a>(
        &'a self,
        context: &'a RuntimeCursorContext,
        plaintext: &'a str,
        limits: RuntimeCursorProtectionLimits,
    ) -> BoxFuture<'a, Result<RuntimeSealedCursor, RuntimeCursorProtectionError>>;
    fn open<'a>(
        &'a self,
        context: &'a RuntimeCursorContext,
        key_id: &'a str,
        ciphertext: &'a [u8],
        limits: RuntimeCursorProtectionLimits,
    ) -> BoxFuture<'a, Result<String, RuntimeCursorProtectionError>>;
}
fn invalid() -> RuntimeGraphqlError {
    RuntimeGraphqlError::new("invalid_cursor")
}
fn validate_key(
    key: &str,
    limits: RuntimeCursorProtectionLimits,
) -> Result<(), RuntimeGraphqlError> {
    if key.is_empty()
        || key.len() > limits.max_key_id_bytes
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid());
    }
    Ok(())
}
pub(super) fn validate_token(
    token: &str,
    limits: RuntimeCursorProtectionLimits,
) -> Result<(&str, &str), RuntimeGraphqlError> {
    if token.len() > limits.max_token_bytes {
        return Err(invalid());
    }
    let mut pieces = token.split('.');
    if pieces.next() != Some("gormgqlc1") {
        return Err(invalid());
    }
    let key = pieces.next().ok_or_else(invalid)?;
    let encoded = pieces.next().ok_or_else(invalid)?;
    if pieces.next().is_some() || encoded.is_empty() {
        return Err(invalid());
    }
    validate_key(key, limits)?;
    let max = limits
        .max_plaintext_bytes
        .checked_add(limits.max_overhead_bytes)
        .and_then(|n| n.checked_add(2))
        .and_then(|n| n.checked_div(3))
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(invalid)?;
    if encoded.len() > max
        || !encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid());
    }
    Ok((key, encoded))
}
pub(super) async fn open(
    provider: &dyn RuntimeCursorProtector,
    context: &RuntimeCursorContext,
    token: &str,
    limits: RuntimeCursorProtectionLimits,
) -> Result<String, RuntimeGraphqlError> {
    let (key, encoded) = validate_token(token, limits)?;
    let ciphertext = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
    let max = limits
        .max_plaintext_bytes
        .checked_add(limits.max_overhead_bytes)
        .ok_or_else(invalid)?;
    if ciphertext.len() > max {
        return Err(invalid());
    }
    let plaintext = provider.open(context, key, &ciphertext, limits).await?;
    if plaintext.len() > limits.max_plaintext_bytes {
        return Err(invalid());
    }
    Ok(plaintext)
}
pub(super) async fn seal(
    provider: &dyn RuntimeCursorProtector,
    context: &RuntimeCursorContext,
    plaintext: &str,
    limits: RuntimeCursorProtectionLimits,
) -> Result<String, RuntimeGraphqlError> {
    if plaintext.len() > limits.max_plaintext_bytes {
        return Err(invalid());
    }
    let sealed = provider.seal(context, plaintext, limits).await?;
    sealed.validate(limits)?;
    if sealed.ciphertext.len()
        > plaintext
            .len()
            .checked_add(limits.max_overhead_bytes)
            .ok_or_else(invalid)?
    {
        return Err(invalid());
    }
    let encoded_size = sealed
        .ciphertext
        .len()
        .checked_add(2)
        .and_then(|n| n.checked_div(3))
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(invalid)?;
    let size = 11usize
        .checked_add(sealed.key_id.len())
        .and_then(|n| n.checked_add(encoded_size))
        .ok_or_else(invalid)?;
    if size > limits.max_token_bytes {
        return Err(invalid());
    }
    Ok(format!(
        "gormgqlc1.{}.{}",
        sealed.key_id,
        URL_SAFE_NO_PAD.encode(sealed.ciphertext)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chacha20poly1305::{
        ChaCha20Poly1305, KeyInit,
        aead::{Aead, Payload},
    };
    struct AeadProvider;
    impl RuntimeCursorProtector for AeadProvider {
        fn seal<'a>(
            &'a self,
            context: &'a RuntimeCursorContext,
            plaintext: &'a str,
            limits: RuntimeCursorProtectionLimits,
        ) -> BoxFuture<'a, Result<RuntimeSealedCursor, RuntimeCursorProtectionError>> {
            Box::pin(async move {
                let cipher = ChaCha20Poly1305::new(&[7; 32].into());
                let nonce_bytes = uuid::Uuid::new_v4();
                let nonce = &nonce_bytes.as_bytes()[..12];
                let mut bytes = nonce.to_vec();
                bytes.extend(
                    cipher
                        .encrypt(
                            nonce.into(),
                            Payload {
                                msg: plaintext.as_bytes(),
                                aad: context.associated_data(),
                            },
                        )
                        .map_err(|_| RuntimeCursorProtectionError::InvalidCursor)?,
                );
                RuntimeSealedCursor::new("active", bytes, limits)
            })
        }
        fn open<'a>(
            &'a self,
            context: &'a RuntimeCursorContext,
            key_id: &'a str,
            ciphertext: &'a [u8],
            _: RuntimeCursorProtectionLimits,
        ) -> BoxFuture<'a, Result<String, RuntimeCursorProtectionError>> {
            Box::pin(async move {
                if key_id != "active" {
                    return Err(RuntimeCursorProtectionError::CursorUnavailable);
                }
                if ciphertext.len() < 28 {
                    return Err(RuntimeCursorProtectionError::InvalidCursor);
                }
                let cipher = ChaCha20Poly1305::new(&[7; 32].into());
                let bytes = cipher
                    .decrypt(
                        (&ciphertext[..12]).into(),
                        Payload {
                            msg: &ciphertext[12..],
                            aad: context.associated_data(),
                        },
                    )
                    .map_err(|_| RuntimeCursorProtectionError::InvalidCursor)?;
                String::from_utf8(bytes).map_err(|_| RuntimeCursorProtectionError::InvalidCursor)
            })
        }
    }
    #[tokio::test]
    async fn complete_envelope_is_encrypted_authenticated_bounded_and_scope_bound() {
        let limits = RuntimeCursorProtectionLimits::default();
        let audience = RuntimeCursorAudience::new("test").unwrap();
        let context =
            RuntimeCursorContext::new(&audience, "tenant-a", &[b"parent-secret"], limits).unwrap();
        let text = "gormrr1.hidden-order-value.hidden-parent-value";
        let token = seal(&AeadProvider, &context, text, limits).await.unwrap();
        assert!(!token.contains("hidden"));
        assert_eq!(
            open(&AeadProvider, &context, &token, limits).await.unwrap(),
            text
        );
        let wrong =
            RuntimeCursorContext::new(&audience, "tenant-b", &[b"parent-secret"], limits).unwrap();
        assert_eq!(
            open(&AeadProvider, &wrong, &token, limits)
                .await
                .unwrap_err()
                .code(),
            "invalid_cursor"
        );
        assert!(open(&AeadProvider, &context, text, limits).await.is_err());
        let (_, encoded) = validate_token(&token, limits).unwrap();
        let mut bytes = URL_SAFE_NO_PAD.decode(encoded).unwrap();
        bytes[20] ^= 1;
        let tampered = format!("gormgqlc1.active.{}", URL_SAFE_NO_PAD.encode(bytes));
        assert_eq!(
            open(&AeadProvider, &context, &tampered, limits)
                .await
                .unwrap_err()
                .code(),
            "invalid_cursor"
        );
        assert_eq!(
            open(
                &AeadProvider,
                &context,
                &token.replace("active", "retired"),
                limits
            )
            .await
            .unwrap_err()
            .code(),
            "cursor_unavailable"
        );
        assert!(
            validate_token(
                &token,
                RuntimeCursorProtectionLimits {
                    max_token_bytes: 10,
                    ..limits
                }
            )
            .is_err()
        );
    }
    struct Rotating {
        active: std::sync::atomic::AtomicU8,
        retired: std::sync::atomic::AtomicBool,
    }
    impl RuntimeCursorProtector for Rotating {
        fn seal<'a>(
            &'a self,
            context: &'a RuntimeCursorContext,
            plaintext: &'a str,
            limits: RuntimeCursorProtectionLimits,
        ) -> BoxFuture<'a, Result<RuntimeSealedCursor, RuntimeCursorProtectionError>> {
            Box::pin(async move {
                let active = self.active.load(std::sync::atomic::Ordering::SeqCst);
                let cipher = ChaCha20Poly1305::new(&[active; 32].into());
                let nonce = uuid::Uuid::new_v4();
                let nonce = &nonce.as_bytes()[..12];
                let mut bytes = nonce.to_vec();
                bytes.extend(
                    cipher
                        .encrypt(
                            nonce.into(),
                            Payload {
                                msg: plaintext.as_bytes(),
                                aad: context.associated_data(),
                            },
                        )
                        .map_err(|_| RuntimeCursorProtectionError::InvalidCursor)?,
                );
                RuntimeSealedCursor::new(format!("key{active}"), bytes, limits)
            })
        }
        fn open<'a>(
            &'a self,
            context: &'a RuntimeCursorContext,
            key_id: &'a str,
            bytes: &'a [u8],
            _: RuntimeCursorProtectionLimits,
        ) -> BoxFuture<'a, Result<String, RuntimeCursorProtectionError>> {
            Box::pin(async move {
                let key = match key_id {
                    "key1" if !self.retired.load(std::sync::atomic::Ordering::SeqCst) => 1,
                    "key2" => 2,
                    _ => return Err(RuntimeCursorProtectionError::CursorUnavailable),
                };
                if bytes.len() < 28 {
                    return Err(RuntimeCursorProtectionError::InvalidCursor);
                }
                let cipher = ChaCha20Poly1305::new(&[key; 32].into());
                let value = cipher
                    .decrypt(
                        (&bytes[..12]).into(),
                        Payload {
                            msg: &bytes[12..],
                            aad: context.associated_data(),
                        },
                    )
                    .map_err(|_| RuntimeCursorProtectionError::InvalidCursor)?;
                String::from_utf8(value).map_err(|_| RuntimeCursorProtectionError::InvalidCursor)
            })
        }
    }
    #[tokio::test]
    async fn rotation_resumes_allowed_old_keys_and_retirement_fails_closed() {
        use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
        let ring = Rotating {
            active: AtomicU8::new(1),
            retired: AtomicBool::new(false),
        };
        let limits = RuntimeCursorProtectionLimits::default();
        let context = RuntimeCursorContext::new(
            &RuntimeCursorAudience::new("rotation").unwrap(),
            "partition",
            &[b"schema/order/parent"],
            limits,
        )
        .unwrap();
        let old = seal(&ring, &context, "entire-internal-envelope", limits)
            .await
            .unwrap();
        ring.active.store(2, Ordering::SeqCst);
        let new = seal(&ring, &context, "entire-internal-envelope", limits)
            .await
            .unwrap();
        assert!(old.starts_with("gormgqlc1.key1."));
        assert!(new.starts_with("gormgqlc1.key2."));
        assert_eq!(
            open(&ring, &context, &old, limits).await.unwrap(),
            "entire-internal-envelope"
        );
        ring.retired.store(true, Ordering::SeqCst);
        assert_eq!(
            open(&ring, &context, &old, limits)
                .await
                .unwrap_err()
                .code(),
            "cursor_unavailable"
        );
        assert_eq!(
            open(&ring, &context, &new, limits).await.unwrap(),
            "entire-internal-envelope"
        );
        assert!(
            seal(
                &ring,
                &context,
                "too-long",
                RuntimeCursorProtectionLimits {
                    max_plaintext_bytes: 2,
                    ..limits
                }
            )
            .await
            .is_err()
        );
        assert!(
            seal(
                &ring,
                &context,
                "ok",
                RuntimeCursorProtectionLimits {
                    max_overhead_bytes: 1,
                    ..limits
                }
            )
            .await
            .is_err()
        );
        assert!(
            RuntimeCursorContext::new(
                &RuntimeCursorAudience::new("rotation").unwrap(),
                "partition",
                &[],
                RuntimeCursorProtectionLimits {
                    max_associated_data_bytes: 8,
                    ..limits
                }
            )
            .is_err()
        );
    }
}
